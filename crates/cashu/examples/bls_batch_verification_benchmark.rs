//! Bounded BLS batch verification measurements with deterministic, non-custodial fixtures.
//!
//! Pass one JSON manifest containing `mode` (`shares`, `proofs`, or `aggregation`), `shape`
//! (`single`, `wallet`, or `fragmented`), `iterations`, and `control_dir`.
//! Fixture construction and warmup precede `ready.json`. A controller writes
//! `start`, samples resources through `results.json`, then writes `release`.
//! Aggregation also requires explicit public `signer_ids` (up to twenty); other
//! modes omit that field. Both waits are bounded. Results contain monotonic
//! durations and public workload metadata, never secrets.

use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use cashu::dhke::batch_verify_bls_messages;
use cashu::nuts::nut01::{
    aggregate_blind_signature_shares, batch_verify_blind_signature_shares, trusted_dealer_keygen,
    verify_blind_signature_share, BlsBlindSignatureShare, BlsG1PublicKey, BlsPublicKeyShare,
    BlsSecretKey, BlsSecretKeyShare, BlsSignerId, PublicKey,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Key(#[from] cashu::nuts::nut01::Error),
    #[error(transparent)]
    Crypto(#[from] cashu::dhke::Error),
    #[error("{0}")]
    Input(&'static str),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Shares,
    Proofs,
    Aggregation,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Shape {
    Single,
    Wallet,
    Fragmented,
}

impl Shape {
    fn key_indexes(self) -> Vec<usize> {
        match self {
            Self::Single => vec![0],
            // The measured 32-sat wallet output shape: 1/1/1/1/2/2/2/2/4/4/4/8.
            Self::Wallet => vec![0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 3],
            Self::Fragmented => (0..64).map(|index| index % 8).collect(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    mode: Mode,
    shape: Shape,
    iterations: usize,
    control_dir: PathBuf,
    #[serde(default)]
    signer_ids: Vec<u16>,
}

struct Fixture {
    messages: Vec<Vec<u8>>,
    blinded_messages: Vec<BlsG1PublicKey>,
    shares: Vec<BlsBlindSignatureShare>,
    public_shares: Vec<BlsPublicKeyShare>,
    signatures: Vec<PublicKey>,
    public_keys: Vec<PublicKey>,
    aggregation_shares: Vec<Vec<BlsBlindSignatureShare>>,
    expected_aggregates: Vec<BlsG1PublicKey>,
}

impl Fixture {
    fn new(indexes: &[usize], signer_ids: &[BlsSignerId]) -> Result<Self, Error> {
        let signer = BlsSignerId::new(1)?;
        let keys = (1..=8)
            .map(|seed| {
                BlsSecretKeyShare::new(signer, BlsSecretKey::from_reduced_bytes(&[seed; 32]))
            })
            .collect::<Vec<_>>();
        let mut fixture = Self {
            messages: Vec::new(),
            blinded_messages: Vec::new(),
            shares: Vec::new(),
            public_shares: Vec::new(),
            signatures: Vec::new(),
            public_keys: Vec::new(),
            aggregation_shares: Vec::new(),
            expected_aggregates: Vec::new(),
        };
        // Non-custodial polynomials exercise distinct shares. Preparation, direct
        // reference signing, and individual share checks are outside timing.
        let aggregation_keys = (0..if signer_ids.is_empty() { 0 } else { 8 })
            .map(|key_index| {
                let coefficients = (0..signer_ids.len())
                    .map(|degree| {
                        BlsSecretKey::from_reduced_bytes(&[(key_index * 20 + degree + 1) as u8; 32])
                    })
                    .collect::<Vec<_>>();
                let keyset = trusted_dealer_keygen(&coefficients, signer_ids)?;
                Ok((coefficients, keyset))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        for (index, key_index) in indexes.iter().enumerate() {
            let message = format!("cdk-bls-batch-benchmark-fixture-{index}").into_bytes();
            let blinded = BlsG1PublicKey::hash_to_curve(&message);
            let key = &keys[*key_index];
            let share = key.blind_sign_share(&blinded);
            let public_share = key.public_key_share();
            fixture.messages.push(message);
            fixture.blinded_messages.push(blinded);
            fixture.signatures.push(share.signature().into());
            fixture.public_keys.push(public_share.public_key().into());
            fixture.shares.push(share);
            fixture.public_shares.push(public_share);
            if let Some((coefficients, keyset)) = aggregation_keys.get(*key_index) {
                let shares = keyset
                    .secret_shares()
                    .iter()
                    .map(|key| key.blind_sign_share(&blinded))
                    .collect::<Vec<_>>();
                for (share, public) in shares.iter().zip(keyset.public_shares()) {
                    if !verify_blind_signature_share(share, &blinded, public)? {
                        return Err(Error::Input("invalid aggregation fixture share"));
                    }
                }
                fixture
                    .expected_aggregates
                    .push(blinded.mul(&coefficients[0]));
                fixture.aggregation_shares.push(shares);
            }
        }
        Ok(fixture)
    }

    fn verify(&self, mode: Mode, messages: &[&[u8]]) -> Result<(), Error> {
        match mode {
            Mode::Shares => {
                if !batch_verify_blind_signature_shares(
                    black_box(&self.shares),
                    black_box(&self.blinded_messages),
                    black_box(&self.public_shares),
                )? {
                    return Err(Error::Input("valid share fixture rejected"));
                }
            }
            Mode::Aggregation => {
                for (shares, expected) in self
                    .aggregation_shares
                    .iter()
                    .zip(&self.expected_aggregates)
                {
                    let aggregate = aggregate_blind_signature_shares(
                        black_box(shares.len()),
                        black_box(shares),
                    )?;
                    if black_box(aggregate) != *expected {
                        return Err(Error::Input(
                            "aggregate differs from direct reference signature",
                        ));
                    }
                }
            }
            Mode::Proofs => batch_verify_bls_messages(
                black_box(&self.public_keys),
                black_box(&self.signatures),
                black_box(messages),
            )?,
        }
        Ok(())
    }
}

fn wait_for(path: &Path) -> Result<(), Error> {
    let started = Instant::now();
    while !path.try_exists()? {
        if started.elapsed() > Duration::from_secs(120) {
            return Err(Error::Input("controller handshake timed out"));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn main() -> Result<(), Error> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or(Error::Input("provide one manifest"))?;
    if args.next().is_some() {
        return Err(Error::Input("provide only one manifest"));
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)?;
    if !(1..=10_000).contains(&manifest.iterations) {
        return Err(Error::Input("iterations must be between 1 and 10000"));
    }
    match manifest.mode {
        Mode::Aggregation if !(1..=20).contains(&manifest.signer_ids.len()) => {
            return Err(Error::Input(
                "aggregation requires between 1 and 20 signer IDs",
            ));
        }
        Mode::Shares | Mode::Proofs if !manifest.signer_ids.is_empty() => {
            return Err(Error::Input("signer IDs apply only to aggregation"));
        }
        _ => {}
    }
    let signer_ids = manifest
        .signer_ids
        .iter()
        .copied()
        .map(BlsSignerId::new)
        .collect::<Result<Vec<_>, _>>()?;
    fs::create_dir(&manifest.control_dir)?;
    let indexes = manifest.shape.key_indexes();
    let fixture = Fixture::new(&indexes, &signer_ids)?;
    let messages = fixture
        .messages
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    for _ in 0..5 {
        fixture.verify(manifest.mode, &messages)?;
    }
    let distinct_keys = indexes
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    fs::write(
        manifest.control_dir.join("ready.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2, "mode": manifest.mode, "shape": manifest.shape,
            "signer_ids": manifest.signer_ids, "shares_per_output": match manifest.mode {
                Mode::Shares => 1, Mode::Proofs => 0, Mode::Aggregation => signer_ids.len(),
            },
            "items": indexes.len(), "distinct_keys": distinct_keys,
            "iterations": manifest.iterations, "warmup_batches": 5,
        }))?,
    )?;
    let mut durations_ns = Vec::with_capacity(manifest.iterations);
    wait_for(&manifest.control_dir.join("start"))?;
    let started = Instant::now();
    for _ in 0..manifest.iterations {
        let batch_started = Instant::now();
        fixture.verify(manifest.mode, &messages)?;
        durations_ns.push(batch_started.elapsed().as_nanos());
    }
    let elapsed_ns = started.elapsed().as_nanos();
    fs::write(
        manifest.control_dir.join("results.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2, "mode": manifest.mode, "shape": manifest.shape,
            "signer_ids": manifest.signer_ids, "shares_per_output": match manifest.mode {
                Mode::Shares => 1, Mode::Proofs => 0, Mode::Aggregation => signer_ids.len(),
            },
            "items": indexes.len(), "distinct_keys": distinct_keys,
            "iterations": manifest.iterations, "all_batches_verified": true,
            "elapsed_ns": elapsed_ns, "durations_ns": durations_ns,
        }))?,
    )?;
    wait_for(&manifest.control_dir.join("release"))
}
