#![allow(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::hint::black_box;
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationDkgCeremonyId, FederationDkgDriverRecords,
    FederationDkgFinalizedMemberConfig, FederationDkgKeysetSpec, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest, FederationDkgSetupState,
    FederationDkgTranscriptSignature, FederationDkgTranscriptSignatureRequest, MemberConfigPrivate,
    MemberConfigPublic, MemberId, ThresholdParams,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::evaluate_public_key_commitments;
use cdk_common::{Amount, CurrencyUnit, SecretKey};
use serde::Serialize;

const HARNESS_VERSION: u16 = 1;
const ALL_PHASES: [&str; 10] = [
    "participant_generation",
    "participant_reload_validation",
    "planner_unchanged",
    "planner_changed_record",
    "commitment_ingestion",
    "reveal_ingestion",
    "duplicate_ingestion",
    "secret_share_verification",
    "local_keyset_derivation",
    "transcript_derivation",
];

#[derive(Debug, Clone)]
struct Args {
    profile: Profile,
    denomination_count: usize,
    iterations: usize,
    timeout: Duration,
    phases: BTreeSet<String>,
    include_full_ceremony: bool,
    pretty: bool,
}

#[derive(Debug, Clone, Copy)]
struct Profile {
    name: &'static str,
    members: u16,
    threshold: u16,
}

impl Profile {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "5-of-3" => Ok(Self {
                name: "5-of-3",
                members: 5,
                threshold: 3,
            }),
            "11-of-8" => Ok(Self {
                name: "11-of-8",
                members: 11,
                threshold: 8,
            }),
            "20-of-14" => Ok(Self {
                name: "20-of-14",
                members: 20,
                threshold: 14,
            }),
            _ => bail!("unknown profile {value}; expected 5-of-3, 11-of-8, or 20-of-14"),
        }
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    harness_version: u16,
    profile: String,
    member_count: u16,
    signature_threshold: u16,
    denomination_count: usize,
    build_mode: &'static str,
    iterations: usize,
    timeout_secs: u64,
    fixture_setup_us: u64,
    complete_fixture_setup_us: u64,
    phases: Vec<PhaseReport>,
    transport: TransportMetrics,
    records: RecordMetrics,
    correctness: CorrectnessReport,
}

#[derive(Debug, Serialize)]
struct PhaseReport {
    name: String,
    samples_us: Vec<u64>,
    median_us: u64,
    min_us: u64,
    max_us: u64,
}

#[derive(Debug, Serialize)]
struct TransportMetrics {
    mode: &'static str,
    rest_attempts: u64,
    rest_successes: u64,
    rest_timeouts: u64,
    duplicate_requests: u64,
    estimated_bytes_sent: u64,
}

#[derive(Debug, Serialize)]
struct RecordMetrics {
    commitments_per_member: usize,
    reveals_per_member: usize,
    secret_share_contributions_per_member: usize,
    keyset_shares_per_member: usize,
    transcript_signatures_per_member: usize,
    serialized_participant_state_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct CorrectnessReport {
    evaluated: bool,
    all_members_complete: bool,
    transcript_hash_equal: bool,
    keyset_id_equal: bool,
    aggregate_keys_equal: bool,
    public_share_mapping_equal: bool,
    local_secret_public_correspondence: bool,
}

impl CorrectnessReport {
    fn skipped() -> Self {
        Self {
            evaluated: false,
            all_members_complete: false,
            transcript_hash_equal: false,
            keyset_id_equal: false,
            aggregate_keys_equal: false,
            public_share_mapping_equal: false,
            local_secret_public_correspondence: false,
        }
    }
}

struct BaseFixture {
    public_config: FederationConfig,
    private_configs: BTreeMap<MemberId, MemberConfigPrivate>,
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: FederationDkgKeysetSpec,
}

struct ParticipantFixture {
    participants: BTreeMap<MemberId, FederationDkgParticipantState>,
    commitments: Vec<FederationDkgParticipantCommitment>,
    reveals: Vec<FederationDkgParticipantCommitmentReveal>,
    contributions: BTreeMap<MemberId, Vec<FederationDkgSecretShareRequest>>,
    serialized_state_bytes: u64,
}

struct CompleteFixture {
    keyset_shares: BTreeMap<MemberId, FederationDkgMemberKeysetShare>,
    transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    correctness: CorrectnessReport,
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error:#}");
            print_usage();
            std::process::exit(2);
        }
    };
    let timeout = args.timeout;
    let pretty = args.pretty;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(run_benchmark(&args));
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(report)) => {
            let json = match pretty {
                true => serde_json::to_string_pretty(&report),
                false => serde_json::to_string(&report),
            }
            .expect("benchmark report serialization");
            println!("{json}");
        }
        Ok(Err(error)) => {
            eprintln!("DKG benchmark failed: {error:#}");
            std::process::exit(1);
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!("DKG benchmark exceeded {} seconds", timeout.as_secs());
            std::process::exit(124);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!("DKG benchmark worker terminated without a report");
            std::process::exit(1);
        }
    }
}

fn parse_args() -> Result<Args> {
    let mut profile = Profile::parse("5-of-3")?;
    let mut denomination_count = 1usize;
    let mut iterations = 3usize;
    let mut timeout = Duration::from_secs(300);
    let mut phases = ALL_PHASES
        .iter()
        .map(|phase| (*phase).to_string())
        .collect::<BTreeSet<_>>();
    let mut include_full_ceremony = false;
    let mut pretty = false;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--profile" => {
                profile = Profile::parse(&args.next().context("--profile requires a value")?)?;
            }
            "--denominations" => {
                denomination_count = match args
                    .next()
                    .context("--denominations requires one or eight")?
                    .as_str()
                {
                    "one" | "1" => 1,
                    "eight" | "8" => 8,
                    value => bail!("invalid denomination profile {value}; expected one or eight"),
                };
            }
            "--iterations" => {
                iterations = args
                    .next()
                    .context("--iterations requires a value")?
                    .parse()
                    .context("invalid iteration count")?;
                if iterations == 0 {
                    bail!("iterations must be greater than zero");
                }
            }
            "--timeout-secs" => {
                let seconds = args
                    .next()
                    .context("--timeout-secs requires a value")?
                    .parse::<u64>()
                    .context("invalid timeout")?;
                if seconds == 0 {
                    bail!("timeout must be greater than zero");
                }
                timeout = Duration::from_secs(seconds);
            }
            "--phases" => {
                let value = args.next().context("--phases requires a value")?;
                phases = if value == "all" {
                    ALL_PHASES
                        .iter()
                        .map(|phase| (*phase).to_string())
                        .collect()
                } else {
                    value.split(',').map(str::to_string).collect()
                };
                for phase in &phases {
                    if !ALL_PHASES.contains(&phase.as_str()) {
                        bail!("unknown phase {phase}");
                    }
                }
            }
            "--full-ceremony" => include_full_ceremony = true,
            "--pretty" => pretty = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => bail!("unknown argument {argument}"),
        }
    }
    Ok(Args {
        profile,
        denomination_count,
        iterations,
        timeout,
        phases,
        include_full_ceremony,
        pretty,
    })
}

fn print_usage() {
    eprintln!(
        "usage: dkg_performance [--profile 5-of-3|11-of-8|20-of-14] \
         [--denominations one|eight] [--iterations N] [--timeout-secs N] \
         [--phases all|PHASE,...] [--full-ceremony] [--pretty]"
    );
}

fn run_benchmark(args: &Args) -> Result<BenchmarkReport> {
    let base = BaseFixture::new(args.profile, args.denomination_count)?;
    let needs_participants = args
        .phases
        .iter()
        .any(|phase| phase != "participant_generation");
    let fixture_start = Instant::now();
    let participant_fixture = needs_participants
        .then(|| ParticipantFixture::new(&base))
        .transpose()?;
    let fixture_setup_us = duration_us(fixture_start.elapsed());
    let needs_complete = args.phases.contains("planner_unchanged")
        || args.phases.contains("planner_changed_record")
        || args.phases.contains("transcript_derivation")
        || args.include_full_ceremony;
    let complete_start = Instant::now();
    let complete_fixture = match (needs_complete, participant_fixture.as_ref()) {
        (true, Some(fixture)) => Some(CompleteFixture::new(&base, fixture)?),
        (true, None) => {
            let fixture = ParticipantFixture::new(&base)?;
            Some(CompleteFixture::new(&base, &fixture)?)
        }
        (false, _) => None,
    };
    let complete_fixture_setup_us = duration_us(complete_start.elapsed());
    let mut reports = Vec::new();

    if args.phases.contains("participant_generation") {
        reports.push(measure_phase(
            "participant_generation",
            args.iterations,
            || generate_participants(&base).map(|states| states.len()),
        )?);
    }
    if let Some(fixture) = participant_fixture.as_ref() {
        if args.phases.contains("participant_reload_validation") {
            let serialized = fixture
                .participants
                .values()
                .map(serde_json::to_vec)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            reports.push(measure_phase(
                "participant_reload_validation",
                args.iterations,
                || {
                    for encoded in &serialized {
                        let state: FederationDkgParticipantState = serde_json::from_slice(encoded)?;
                        state.validate(&base.public_config)?;
                        black_box(state);
                    }
                    Ok(serialized.len())
                },
            )?);
        }
        if args.phases.contains("commitment_ingestion") {
            reports.push(measure_phase(
                "commitment_ingestion",
                args.iterations,
                || ingest_commitments(&base.public_config, &fixture.commitments),
            )?);
        }
        if args.phases.contains("reveal_ingestion") {
            reports.push(measure_phase("reveal_ingestion", args.iterations, || {
                ingest_reveals(&base.public_config, fixture)
            })?);
        }
        if args.phases.contains("duplicate_ingestion") {
            reports.push(measure_phase(
                "duplicate_ingestion",
                args.iterations,
                || ingest_duplicates(&base.public_config, fixture),
            )?);
        }
        if args.phases.contains("secret_share_verification") {
            reports.push(measure_phase(
                "secret_share_verification",
                args.iterations,
                || verify_received_secret_shares(&base.public_config, fixture),
            )?);
        }
        if args.phases.contains("local_keyset_derivation") {
            reports.push(measure_phase(
                "local_keyset_derivation",
                args.iterations,
                || derive_all_member_keyset_shares(&base.public_config, fixture),
            )?);
        }
    }
    if let Some(complete) = complete_fixture.as_ref() {
        let local_member = *base
            .public_config
            .members
            .keys()
            .next()
            .context("empty federation")?;
        let participant = participant_fixture
            .as_ref()
            .and_then(|fixture| fixture.participants.get(&local_member))
            .context("missing local participant fixture")?;
        let keyset_share = complete
            .keyset_shares
            .get(&local_member)
            .context("missing local keyset share")?;
        let private_config = base
            .private_configs
            .get(&local_member)
            .context("missing local private config")?;
        let complete_records = complete_records_for(
            participant_fixture
                .as_ref()
                .context("missing participant fixture")?,
            local_member,
            keyset_share,
            &complete.transcript_signatures,
        );
        if args.phases.contains("planner_unchanged") {
            reports.push(measure_phase("planner_unchanged", args.iterations, || {
                complete_records
                    .plan(&base.public_config, private_config, participant)
                    .map_err(Into::into)
            })?);
        }
        if args.phases.contains("planner_changed_record") {
            let mut changed_records = complete_records.clone();
            let last_signature = changed_records
                .transcript_signatures
                .pop()
                .context("missing changed transcript signature")?;
            reports.push(measure_phase(
                "planner_changed_record",
                args.iterations,
                || {
                    let mut records = changed_records.clone();
                    records.transcript_signatures.push(last_signature);
                    records
                        .plan(&base.public_config, private_config, participant)
                        .map_err(Into::into)
                },
            )?);
        }
        if args.phases.contains("transcript_derivation") {
            reports.push(measure_phase(
                "transcript_derivation",
                args.iterations,
                || {
                    FederationDkgFinalizedMemberConfig::from_records(
                        &base.public_config,
                        private_config,
                        std::slice::from_ref(keyset_share),
                        &complete.transcript_signatures,
                    )
                    .map_err(Into::into)
                },
            )?);
        }
    }
    if args.include_full_ceremony {
        reports.push(measure_phase(
            "complete_in_memory_ceremony",
            args.iterations,
            || run_complete_ceremony(&base),
        )?);
    }

    let fixture = participant_fixture.as_ref();
    let transport = transport_metrics(
        args.profile.members,
        fixture,
        complete_fixture.as_ref(),
        args.phases.contains("duplicate_ingestion"),
    )?;
    let fixture_member_count = fixture.map_or(0, |_| usize::from(args.profile.members));
    let records = RecordMetrics {
        commitments_per_member: fixture_member_count,
        reveals_per_member: fixture_member_count,
        secret_share_contributions_per_member: fixture_member_count,
        keyset_shares_per_member: usize::from(complete_fixture.is_some()),
        transcript_signatures_per_member: complete_fixture
            .as_ref()
            .map_or(0, |complete| complete.transcript_signatures.len()),
        serialized_participant_state_bytes: fixture.map_or(0, |value| value.serialized_state_bytes),
    };
    Ok(BenchmarkReport {
        harness_version: HARNESS_VERSION,
        profile: args.profile.name.to_string(),
        member_count: args.profile.members,
        signature_threshold: args.profile.threshold,
        denomination_count: args.denomination_count,
        build_mode: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        iterations: args.iterations,
        timeout_secs: args.timeout.as_secs(),
        fixture_setup_us,
        complete_fixture_setup_us,
        phases: reports,
        transport,
        records,
        correctness: complete_fixture
            .map_or_else(CorrectnessReport::skipped, |complete| complete.correctness),
    })
}

impl BaseFixture {
    fn new(profile: Profile, denomination_count: usize) -> Result<Self> {
        let members = (1..=profile.members)
            .map(member_id)
            .map(|member| {
                let public = MemberConfigPublic {
                    member_id: member,
                    public_mint_url: MintUrl::from_str(&format!(
                        "https://mint-{}.example.invalid",
                        member.get()
                    ))?,
                    federation_api_url: MintUrl::from_str(&format!(
                        "https://fed-{}.example.invalid",
                        member.get()
                    ))?,
                    identity_public_key: identity_secret(member)?.public_key(),
                };
                Ok((member, public))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let public_config = FederationConfig::new(
            ThresholdParams::with_bft_consensus(profile.members, profile.threshold)?,
            members,
            Vec::new(),
        )?;
        let private_configs = public_config
            .members
            .keys()
            .copied()
            .map(|member| {
                Ok((
                    member,
                    MemberConfigPrivate {
                        federation_id: Some(public_config.federation_id),
                        member_id: member,
                        identity_secret_key: identity_secret(member)?,
                        keyset_secret_shares: BTreeMap::new(),
                        setup_transcript_hash: None,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let amounts = (0..denomination_count)
            .map(|index| Amount::from(1_u64 << index))
            .collect();
        Ok(Self {
            public_config,
            private_configs,
            ceremony_id: FederationDkgCeremonyId::from_bytes([profile.members as u8; 32]),
            keyset_spec: FederationDkgKeysetSpec::new(CurrencyUnit::Sat, true, 0, None, amounts)?,
        })
    }
}

impl ParticipantFixture {
    fn new(base: &BaseFixture) -> Result<Self> {
        let participants = generate_participants(base)?;
        let commitments = participants
            .values()
            .map(|participant| Ok(participant.commitment_request()?.commitment))
            .collect::<Result<Vec<_>>>()?;
        let reveals = participants
            .values()
            .map(|participant| participant.reveal_request().reveal)
            .collect::<Vec<_>>();
        let contributions = base
            .public_config
            .members
            .keys()
            .copied()
            .map(|receiver| {
                let requests = participants
                    .values()
                    .map(|participant| participant.secret_share_request(receiver))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok((receiver, requests))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let serialized_state_bytes = participants
            .values()
            .map(serde_json::to_vec)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .map(|encoded| encoded.len() as u64)
            .sum();
        Ok(Self {
            participants,
            commitments,
            reveals,
            contributions,
            serialized_state_bytes,
        })
    }
}

impl CompleteFixture {
    fn new(base: &BaseFixture, fixture: &ParticipantFixture) -> Result<Self> {
        let keyset_shares = derive_all_member_keyset_shares(&base.public_config, fixture)?;
        let transcript_signatures = keyset_shares
            .iter()
            .map(|(member, share)| {
                let hash =
                    FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                        &base.public_config,
                        std::slice::from_ref(share),
                    )?;
                let signature = FederationDkgTranscriptSignature::sign_no_aux_rand(
                    hash,
                    base.private_configs
                        .get(member)
                        .context("missing signer private config")?,
                )?;
                Ok(FederationDkgTranscriptSignatureRequest::new(
                    base.public_config.federation_id,
                    *member,
                    hash,
                    signature,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let correctness = verify_complete_fixture(base, &keyset_shares, &transcript_signatures)?;
        Ok(Self {
            keyset_shares,
            transcript_signatures,
            correctness,
        })
    }
}

fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("benchmark member ID")
}

fn identity_secret(member: MemberId) -> Result<SecretKey> {
    let mut bytes = [0u8; 32];
    bytes[30..].copy_from_slice(&member.get().to_be_bytes());
    SecretKey::from_slice(&bytes).context("benchmark identity key")
}

fn generate_participants(
    base: &BaseFixture,
) -> Result<BTreeMap<MemberId, FederationDkgParticipantState>> {
    base.public_config
        .members
        .keys()
        .copied()
        .map(|member| {
            Ok((
                member,
                FederationDkgParticipantState::generate(
                    &base.public_config,
                    base.ceremony_id,
                    member,
                    base.keyset_spec.clone(),
                )?,
            ))
        })
        .collect()
}

fn ingest_commitments(
    config: &FederationConfig,
    commitments: &[FederationDkgParticipantCommitment],
) -> Result<FederationDkgSetupState> {
    let mut state = FederationDkgSetupState::new(config);
    for commitment in commitments {
        state.insert_commitment(config, *commitment)?;
    }
    Ok(state)
}

fn ingest_reveals(
    config: &FederationConfig,
    fixture: &ParticipantFixture,
) -> Result<FederationDkgSetupState> {
    let mut state = ingest_commitments(config, &fixture.commitments)?;
    for reveal in &fixture.reveals {
        state.insert_reveal(config, reveal.clone())?;
    }
    Ok(state)
}

fn ingest_duplicates(config: &FederationConfig, fixture: &ParticipantFixture) -> Result<usize> {
    let mut state = ingest_reveals(config, fixture)?;
    let mut duplicate_count = 0;
    for commitment in &fixture.commitments {
        if !state.insert_commitment(config, *commitment)? {
            duplicate_count += 1;
        }
    }
    for reveal in &fixture.reveals {
        if !state.insert_reveal(config, reveal.clone())? {
            duplicate_count += 1;
        }
    }
    Ok(duplicate_count)
}

fn verify_received_secret_shares(
    config: &FederationConfig,
    fixture: &ParticipantFixture,
) -> Result<usize> {
    let reveals = fixture
        .reveals
        .iter()
        .map(|reveal| (reveal.member_id, reveal))
        .collect::<BTreeMap<_, _>>();
    let mut verified = 0usize;
    for (receiver, requests) in &fixture.contributions {
        for request in requests {
            request.validate_sender_receiver(config, request.sender, *receiver)?;
            let reveal = reveals
                .get(&request.sender)
                .context("missing sender reveal")?;
            for (amount, secret_share) in &request.secret_shares {
                let commitments = reveal
                    .coefficient_commitments
                    .get(amount)
                    .context("missing amount commitments")?;
                let expected =
                    evaluate_public_key_commitments(receiver.to_bls_signer_id(), commitments)?;
                if secret_share.public_key_share() != expected {
                    bail!("secret contribution did not match public polynomial");
                }
                verified += 1;
            }
        }
    }
    Ok(verified)
}

fn derive_all_member_keyset_shares(
    config: &FederationConfig,
    fixture: &ParticipantFixture,
) -> Result<BTreeMap<MemberId, FederationDkgMemberKeysetShare>> {
    let setup_state = ingest_reveals(config, fixture)?;
    let round = setup_state
        .complete_round(config)?
        .context("complete fixture did not produce a DKG round")?;
    config
        .members
        .keys()
        .copied()
        .map(|member| {
            let incoming = fixture
                .contributions
                .get(&member)
                .context("missing member contributions")?
                .iter()
                .cloned()
                .map(|request| (request.sender, request))
                .collect::<BTreeMap<_, _>>();
            Ok((
                member,
                round.derive_member_keyset_share(
                    config,
                    &setup_state.reveals,
                    member,
                    &incoming,
                )?,
            ))
        })
        .collect()
}

fn complete_records_for(
    fixture: &ParticipantFixture,
    member: MemberId,
    keyset_share: &FederationDkgMemberKeysetShare,
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
) -> FederationDkgDriverRecords {
    FederationDkgDriverRecords {
        commitments: fixture.commitments.clone(),
        reveals: fixture.reveals.clone(),
        secret_share_contributions: fixture
            .contributions
            .get(&member)
            .expect("member contributions")
            .clone(),
        keyset_shares: vec![keyset_share.clone()],
        transcript_signatures: transcript_signatures.to_vec(),
    }
}

fn verify_complete_fixture(
    base: &BaseFixture,
    shares: &BTreeMap<MemberId, FederationDkgMemberKeysetShare>,
    signatures: &[FederationDkgTranscriptSignatureRequest],
) -> Result<CorrectnessReport> {
    let first_share = shares.values().next().context("missing keyset shares")?;
    let transcript_hash_equal = signatures
        .iter()
        .all(|signature| signature.transcript_hash == signatures[0].transcript_hash);
    let keyset_id_equal = shares
        .values()
        .all(|share| share.keyset.id == first_share.keyset.id);
    let aggregate_keys_equal = shares
        .values()
        .all(|share| share.keyset.aggregate_keys == first_share.keyset.aggregate_keys);
    let public_share_mapping_equal = shares
        .values()
        .all(|share| share.keyset.public_shares == first_share.keyset.public_shares);
    let local_secret_public_correspondence = shares.iter().all(|(member, share)| {
        let Some(public_shares) = share.keyset.public_shares.get(member) else {
            return false;
        };
        share.secret_shares.iter().all(|(amount, secret_share)| {
            public_shares.get(amount) == Some(&secret_share.public_key_share())
        })
    });
    let mut finalized_public_config = None;
    for (member, share) in shares {
        let finalized = FederationDkgFinalizedMemberConfig::from_records(
            &base.public_config,
            base.private_configs
                .get(member)
                .context("missing final private config")?,
            std::slice::from_ref(share),
            signatures,
        )?;
        match finalized_public_config.as_ref() {
            Some(expected) if expected != &finalized.public_config => {
                bail!("final public configs diverged")
            }
            Some(_) => {}
            None => finalized_public_config = Some(finalized.public_config),
        }
    }
    Ok(CorrectnessReport {
        evaluated: true,
        all_members_complete: shares.len() == base.public_config.members.len()
            && signatures.len() == base.public_config.members.len(),
        transcript_hash_equal,
        keyset_id_equal,
        aggregate_keys_equal,
        public_share_mapping_equal,
        local_secret_public_correspondence,
    })
}

fn run_complete_ceremony(base: &BaseFixture) -> Result<CorrectnessReport> {
    let fixture = ParticipantFixture::new(base)?;
    Ok(CompleteFixture::new(base, &fixture)?.correctness)
}

fn transport_metrics(
    member_count: u16,
    fixture: Option<&ParticipantFixture>,
    complete: Option<&CompleteFixture>,
    measured_duplicate_ingestion: bool,
) -> Result<TransportMetrics> {
    let members = u64::from(member_count);
    let estimated_bytes_sent = match fixture {
        Some(fixture) => {
            let commitment_bytes = fixture
                .commitments
                .iter()
                .map(serde_json::to_vec)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .map(|encoded| encoded.len() as u64)
                .sum::<u64>()
                * members;
            let reveal_bytes = fixture
                .reveals
                .iter()
                .map(serde_json::to_vec)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .map(|encoded| encoded.len() as u64)
                .sum::<u64>()
                * members;
            let contribution_bytes = fixture
                .contributions
                .values()
                .flatten()
                .map(serde_json::to_vec)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .map(|encoded| encoded.len() as u64)
                .sum::<u64>();
            let signature_bytes = complete
                .map(|complete| {
                    complete
                        .transcript_signatures
                        .iter()
                        .map(serde_json::to_vec)
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map(|encoded| {
                            encoded.iter().map(|value| value.len() as u64).sum::<u64>() * members
                        })
                })
                .transpose()?
                .unwrap_or(0);
            commitment_bytes + reveal_bytes + contribution_bytes + signature_bytes
        }
        None => 0,
    };
    Ok(TransportMetrics {
        mode: "in_memory_phase_harness",
        rest_attempts: 0,
        rest_successes: 0,
        rest_timeouts: 0,
        duplicate_requests: match (fixture.is_some(), measured_duplicate_ingestion) {
            (true, true) => members * 2,
            _ => 0,
        },
        estimated_bytes_sent,
    })
}

fn measure_phase<T, F>(name: &str, iterations: usize, mut operation: F) -> Result<PhaseReport>
where
    F: FnMut() -> Result<T>,
{
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        black_box(operation()?);
        samples.push(duration_us(started.elapsed()));
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    Ok(PhaseReport {
        name: name.to_string(),
        median_us: sorted[sorted.len() / 2],
        min_us: sorted[0],
        max_us: sorted[sorted.len() - 1],
        samples_us: samples,
    })
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
