# Federation presentation notes

This file is a working collection of presentation research, not slides.
Start with the original `bls` branch and the BLS v3 keyset transition.
Federation details come later.

---

## Branch map

Current checkout: `bls-federation`.

Relationship to `main` and the original BLS work:

- `main` is the standalone CDK baseline.
- `bls` is the original BLS integration branch. It sits 10 commits above `main`.
- `bls-federation` is based on the tip of `bls`.
- Merge-base `main` / `bls` / `bls-federation`: `e2c0d0a4aa09c1c7862d16ef0d8968ce95bed65e`
- Merge-base `bls` / `bls-federation`: `eb34616e88c6f50623ec37b94997c67257279693` (`origin/bls`)

Useful related local branches:

- `bls` / `origin/bls` / `bls-sigall`: original BLS integration
- `bls-federation`: current federated Cashu work
- `bls-federation-cleanup`, `bls-federation-cleanup-26-07-29`: cleanup/verification snapshots
- `bls-federation-bdk-frost`: FROST / BDK worktree
- `codex/pr-2048`: `fix: verify v3 BLS blind signatures`

The original BLS precursor is small. `main..bls` is about 47 files and +2463 / -272 lines. `bls..bls-federation` is a much larger later layer (1000+ commits).

Do not mix those two stories in the same chapter.

---

## Naming trap

This will confuse the audience if it is not said first.

| User-facing name | Rust enum | Keyset ID prefix | Meaning |
|---|---|---|---|
| v1 | `KeySetVersion::Version00` | `00` + 7 bytes | original secp256k1 keyset IDs |
| v2 | `KeySetVersion::Version01` | `01` + 32 bytes | secp256k1 keyset IDs that commit to unit / fee / expiry |
| v3 | `KeySetVersion::Version02` | `02` + 32 bytes | BLS12-381 keysets |

In conversation we say "keyset v3" or "BLS v3".
In code the version is `Version02`.
In hex IDs the first byte is `02`.

There is a second naming collision: Cashu already has `TokenV3` / `ProofV3`. Those are token encoding versions, not BLS keyset v3.

---

## Original `bls` commits

`git log --oneline main..bls`:

1. `578e26ebd` `feat: bls` (2026-05-20, thesimplekid)
   The main landing. Adds BLS12-381 types, versioned DHKE, v3 keyset IDs, mint/wallet/signatory/mintd wiring.
2. `1d71add6e` `fix: remove extra hash step` (2026-06-08)
   First v3 mint-key derivation hashed BIP32 child bytes, then reduced them into Fr. The extra SHA-256 was removed.
3. `59a7a80e5` `fix: use rejection sampling` (2026-06-08)
   Mint amount keys and NUT-13 blinding factors stopped using modular reduction and started rejection-sampling canonical non-zero scalars in `Fr*`.
4. `2dedc3ca7` `fix: reject non-secp256k1 pubkeys in P2PK/HTLC to prevent panic` (2026-06-10)
   Mixed-curve `PublicKey` made old secp-only helpers panic on valid BLS points.
5. `5d4d0d7ac` `fix: use rejection sampling for BLS batch weights per NUT-00` (2026-06-10)
   Batch verification weights now match the NUT-00 vector. Modular reduction plus a one-byte counter was wrong.
6. `78189e0c5` `fix: rejection-sample BLS blinding factors in generate_bls` (2026-06-10)
   Random BLS scalars are sampled uniformly from `Fr*`, never reduced raw 32-byte strings.
7. `63dfceed9` `test(fuzz): add BLS12-381 (v3) fuzz targets` (2026-06-10)
   `fuzz_bls_dhke` and `fuzz_bls_pubkey_in_conditions`.
8. `289fe9e44` `fix: verify v3 BLS blind signatures` (2026-07-03, callebtc)
   Wallets must verify the *blind* signature `e(C_, G2) == e(B, K)` before / in addition to the unblinded proof check.
9. `d2025b1c2` Merge main into bls
10. `eb34616e8` Merge branch 'main' into bls

External references:

- PR discussion / review trail: [#2048](https://github.com/cashubtc/cdk/pull/2048)
- Follow-up merge marker: `origin/pr/2194`

---

## Why BLS exists before federation

Standalone Cashu v1/v2 is secp256k1 additive DHKE:

- `Y = hash_to_curve(secret)`
- blind: `B_ = Y + rG`
- sign: `C_ = k · B_`
- unblind: `C = C_ - rK`
- prove mint knowledge of `k` with DLEQ

That works for a single mint operator. It does not give threshold issuance. You cannot split `k` across members and still let a wallet verify an ordinary Cashu proof against one public mint key.

BLS12-381 changes the algebra to a multiplicative pairing scheme:

- mint public keys live in G2 (96-byte compressed)
- blinded messages and signatures live in G1 (48-byte compressed)
- secrets hash to G1 with DST `CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_`
- blind: `B = r · H(secret)`
- sign: `C_ = k · B`
- unblind: `C = r⁻¹ · C_`
- verify proof: `e(C, G2) == e(H(secret), K)`
- verify blind signature: `e(C_, G2) == e(B, K)`

The pairing equation is the reason federation can later issue threshold shares. A wallet checks a signature against the aggregate G2 key and never needs `k`.

Important scope limit: the original `bls` branch is single-signer BLS. It does **not** implement:

- threshold shares
- Lagrange interpolation
- DKG
- federation keyset gates
- member rosters or consensus

Those start on `bls-federation`. On the current checkout, `crates/cashu/src/nuts/nut01/bls.rs` already contains later threshold types (`BlsSignerId`, `BlsSecretKeyShare`, `BlsPublicKeyShare`, `BlsBlindSignatureShare`, `BlsThresholdKeySet`). Those are federation additions, not part of original `bls`.

On `bls`, `nut01/bls.rs` is only ~326 lines and contains:

- `BlsSecretKey`
- `BlsG1PublicKey`
- `BlsG2PublicKey`
- pairing verify
- blind-signature pairing verify
- batch verify with transcript-derived weights

---

## Keyset v3 transition

This is the actual protocol switch. The branch does not invent a new token format. It adds a third keyset version and then makes DHKE, derivation, signing, and verification branch on that version.

### What v3 reuses from v2

`Id::v3_from_data()` is the v2 commitment recipe with the version byte rewritten:

1. Sort public keys by amount.
2. Concatenate `amount:hex(pubkey)` entries.
3. Append `|unit:...`
4. Optionally append `|input_fee_ppk:...`
5. Optionally append `|final_expiry:...`
6. SHA-256 and take 32 hex-decoded bytes / first 31 bytes of the hash, depending on the existing v2 helper.
7. Prefix version byte `02` instead of `01`.

The public-key bytes in that string are compressed BLS G2 keys, not secp keys. Same ID width as v2: 33 bytes total.

Test vectors in `nut02.rs`:

- `BLS_V3_KEYSET_VECTOR_KEY` is a compressed G2 hex string
- no fee / no expiry ID: `02ce4c47836fd0e64f37a08254777b7fd0dedb95fc1ddd0acadf5600674c743c5d`
- fee `100` and expiry `2_000_000_000` ID: `02b532391cadf8c5d98bf0ff05b85e3cfb76a8175d71822140df3396c20cf40588`

### Version matrix

| User-facing | Enum | ID | Curve | Blind | Unblind | Wallet/mint verify | DLEQ |
|---|---|---|---|---|---|---|---|
| v1 | `Version00` | 8 bytes (`00` + 7) | secp256k1 | `Y + rG` | `C_ - rK` | secp multiply / DLEQ | optional |
| v2 | `Version01` | 33 bytes (`01` + 32) | secp256k1 | `Y + rG` | `C_ - rK` | secp multiply / DLEQ | optional |
| v3 | `Version02` | 33 bytes (`02` + 32) | BLS12-381 | `r · H(secret)` | `r⁻¹ · C_` | pairing | forbidden |

### Default mint policy

`MintBuilder::preferred_keyset_version()`:

- `use_keyset_v2 = Some(false)` → force v1 (`Version00`)
- `use_keyset_v2 = Some(true)` → force v2 (`Version01`)
- `use_keyset_v2 = None` → new keysets are v3 (`Version02`)

Existing active keysets are preserved unless an operator explicitly forces v1 or v2. That is a compatibility choice: do not rotate a live secp keyset just because the binary now knows about BLS.

Rotation grew an explicit version argument:

- `Mint::rotate_keyset_by_version(...)`
- fake-wallet rotations accept `"v1" | "v2" | "v3"`
- mint-cli: `rotate-next-keyset --keyset-version v3`

The old boolean `use_keyset_v2` remains as a compatibility switch, but the real control plane is now `KeySetVersion`.

Proto mapping is off-by-one from the on-wire version byte:

- proto `1` = v1 / `Version00`
- proto `2` = v2 / `Version01`
- proto `3` = v3 / `Version02`

On original `bls`, `cdk-signatory` proto `KeysetVersion` still only listed V1/V2 names. The Rust enum mapping already understood proto `3` as v3.

---

## Crypto building blocks

### Point / scalar types

`PublicKey` became a three-variant enum:

- `Secp256k1` — 33-byte compressed
- `BlsG1` — 48-byte compressed, blinded messages and signatures
- `BlsG2` — 96-byte compressed, mint public keys

`SecretKey` became:

- `Secp256k1`
- `Bls` (`BlsSecretKey`)

Parsing is by length. Identity / infinity points are rejected. BLS scalars are stored big-endian on the wire and reversed when talking to `bls12_381::Scalar`.

`SecretKey::public_key()` for a BLS scalar returns the G2 mint key `k · G2`.

Several old secp helpers still panic if they receive a BLS variant:

- `PublicKey::to_uncompressed_bytes()`
- `PublicKey::x_only_public_key()`
- `PublicKey::negate()`
- `SecretKey::to_scalar()` / `as_scalar()`
- `SecretKey::x_only_public_key()`

That panic surface is an original-branch leftover. Later federation cleanup docs call it out explicitly.

### Hash-to-curve

Secp (v1/v2) keeps the old Cashu loop:

- domain separator `Secp256k1_HashToCurve_Cashu_`
- try even x-only keys until one lands on-curve

BLS v3 uses standardized hash-to-curve:

- `BlsG1PublicKey::hash_to_curve(message)`
- DST `CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_`
- `ExpandMsgXmd<Sha256>`

`hash_to_curve_for_version()` is the protocol switch used by `Proof::y()`.

### DHKE

New entry points:

- `blind_message_for_version(secret, r, version)`
- `construct_proofs(...)` branches on `keyset_id.get_version()`
- `sign_message(k, B)` matches on `SecretKey::{Secp256k1, Bls}`
- `verify_bls_message(K, C, secret)`
- `verify_bls_blind_signature(K, C_, B)`
- `verify_bls_message_keyed(k, C, secret)`
- `batch_verify_bls_messages(...)`

v3 DLEQ is rejected in `construct_proofs()`. If a v3 blind signature carries DLEQ data, proof construction fails.

NUT-12 (`calculate_dleq`, `add_dleq_proof`) returns `None` for `Version02`. DLEQ stays secp-only.

### Batch verification

`derive_batch_weights()` follows NUT-00:

1. Transcript prefix `Cashu_BLS_Batch_v1`
2. For each item: `C || K || u32_BE(len(msg)) || msg`
3. `challenge = SHA256(transcript)`
4. For each index `i`, rejection-sample:
   `h = SHA256(challenge || u32_BE(i) || u32_BE(ctr))`
   accept only canonical non-zero `Fr*` scalars

The first implementation reduced bytes and used a one-byte counter. That failed the published NUT-00 vector. The fix is commit `5d4d0d7ac`.

There is also a later federation-era `Cashu_BLS_Blind_Batch_v1` constant on the current branch. That is not part of original `bls`.

---

## Deterministic derivation

### Wallet secrets: NUT-13

`Secret::from_seed`:

- v1: old BIP32 path
- v2 and v3: HMAC-SHA256 KDF `Cashu_KDF_HMAC_SHA256 || keyset_id || u64_BE(counter) || 0x00`

`SecretKey::from_seed` (blinding factor):

- v1: old BIP32 path, child `1`
- v2: same HMAC KDF with suffix `0x01`, parsed as secp
- v3: same HMAC KDF with suffix `0x01`, then an attempt counter, parsed as canonical BLS scalar

v3 blinding-factor loop:

```
message = "Cashu_KDF_HMAC_SHA256" || keyset_id || u64_BE(counter) || 0x01
for attempt in 0..=u32::MAX:
    h = HMAC-SHA256(seed, message || u32_BE(attempt))
    reject all-zero
    accept if h is a canonical BLS scalar
```

`PreMintSecrets::from_seed` and blank change outputs call `blind_message_for_version(..., keyset_id.get_version())`, so restore / deterministic minting automatically follows the keyset curve.

### Mint amount keys: NUT-02 `MintKeySet::generate`

v1/v2: hardened BIP32 child `i` is a secp key.

v3 derivation evolved in three steps:

1. Original `feat: bls`: take BIP32 child bytes, SHA-256 them, reduce into Fr.
2. `1d71add6e`: remove the extra hash; reduce the BIP32 child bytes directly.
3. `59a7a80e5`: rejection-sample a second hardened index `attempt` until the 32 child bytes are a canonical non-zero BLS scalar.

Current v3 mint derivation:

```
for amount index i:
    attempt = 0
    loop:
        child = BIP32(i' / attempt')
        reject all-zero
        accept SecretKey::bls_from_slice(child_bytes)
```

There is a unit test that proves the live key is the rejection-sampled two-level child, not the old reduced one-level child.

Random ephemeral blinding uses `SecretKey::generate_bls()`, which is the same `Fr*` rejection sampler over `OsRng`.

---

## End-to-end v3 proof life cycle

This is the walk that later makes federation obvious.

### Wallet prepares outputs

1. Choose the mint's active keyset. If it is v3, all later crypto is BLS.
2. Derive `secret` and blinding factor `r` from the wallet seed (or generate random ones for non-restore outputs).
3. Blind: `B = r · H(secret)` in G1.
4. Send ordinary Cashu `BlindedMessage`s. The public HTTP/WS protocol does not change.

### Mint / signatory signs

1. Look up the amount key `k` from the v3 keyset.
2. `C_ = k · B` in G1.
3. Do not attach DLEQ.

`cdk-signatory` `verify_proofs()` later checks spends with `verify_bls_message` and rejects leftover DLEQ on v3 proofs.

### Wallet checks the mint response

Correct v3 verification is two-stage:

1. Blind check, before trusting the mint response:
   `e(C_, G2) == e(B, K)`
2. Unblind: `C = r⁻¹ · C_`
3. Proof check:
   `e(C, G2) == e(H(secret), K)`

If a v3 signature or proof still carries DLEQ, reject it. DLEQ is not defined for this curve in NUT-12.

This was originally incomplete. Early wallet code could construct a v3 proof and only check the unblinded pairing, or skip the blind pairing entirely. Commit `289fe9e44` added `verify_bls_blind_signature()` and wired it into:

- `wallet/blind_signature.rs` mint-response validation
- restore
- auth-wallet mint
- token signature verification (`verify_token_signatures`, with `verify_token_dleq` kept as a deprecated alias)

Receive / issue sagas also verify v3 proofs with pairings after unblinding.

### Tokens

v3 proofs are still ordinary Cashu proofs. The token formats did not need a new version.

Two serialization details matter:

- `ProofV4.dleq` is now `skip_serializing_if = "Option::is_none"`
- there is a test that a v3 token does not encode an empty DLEQ (`0x61 0x64 0xf6` / `"d": null`)

`Proof::y()` is version-aware: secp hash-to-curve for v1/v2, BLS G1 hash-to-curve for v3.

---

## Mixed-curve safety

Once public keys can be secp, G1, or G2, several old assumptions break.

P2PK / HTLC remain secp-only. A compressed G1 point is valid hex and valid `PublicKey::from_slice` input, so an attacker can put a BLS point in `nut10` spending-condition data. Before the fix this panicked in `x_only_public_key()`. After the fix it returns `NonSecp256k1Pubkey`.

Fuzzing had to change because random hex almost never decodes to a subgroup-correct BLS point:

- `fuzz/fuzz_targets/fuzz_bls_pubkey_in_conditions.rs`
  Injects *valid* compressed G1/G2 points into P2PK/HTLC `data`, `pubkeys`, and `refund_keys`, then asserts `verify_p2pk` / `verify_htlc` error instead of panic.
- `fuzz/fuzz_targets/fuzz_bls_dhke.rs`
  Panic-safety for G1/G2/scalar parsing, blind/sign/unblind/verify, and batch-weight rejection sampling. Also checks soundness: honest v3 proofs verify, tampering fails.

Other safety details worth keeping:

- identity points rejected on G1 and G2 parse
- zero scalars rejected for blinding factors, mint keys, and batch weights
- NUT-00 batch vector is encoded as a unit test in `nut01/bls.rs`

---

## Files that belong in the BLS / v3 story

Protocol / crypto:

- `crates/cashu/src/nuts/nut01/bls.rs`
- `crates/cashu/src/nuts/nut01/public_key.rs`
- `crates/cashu/src/nuts/nut01/secret_key.rs`
- `crates/cashu/src/nuts/nut01/mod.rs`
- `crates/cashu/src/nuts/nut02.rs`
- `crates/cashu/src/dhke.rs`
- `crates/cashu/src/nuts/nut12.rs`
- `crates/cashu/src/nuts/nut13.rs`
- `crates/cashu/src/nuts/nut00/mod.rs`
- `crates/cashu/src/nuts/nut00/token.rs`
- `crates/cashu/src/nuts/nut11/mod.rs`

Mint / signatory / mintd:

- `crates/cdk/src/mint/builder.rs`
- `crates/cdk/src/mint/keysets/mod.rs`
- `crates/cdk-signatory/src/db_signatory.rs`
- `crates/cdk-mintd/src/config.rs`
- `crates/cdk-mintd/src/lib.rs`
- `crates/cdk-mintd/example.config.toml`
- `crates/cdk-mintd/README.md`

Wallet:

- `crates/cdk/src/wallet/mod.rs`
- `crates/cdk/src/wallet/blind_signature.rs`
- `crates/cdk/src/wallet/issue/saga/mod.rs`
- `crates/cdk/src/wallet/receive/saga/mod.rs`
- `crates/cdk/src/wallet/auth/auth_wallet.rs`

Tests / fuzz:

- `fuzz/fuzz_targets/fuzz_bls_dhke.rs`
- `fuzz/fuzz_targets/fuzz_bls_pubkey_in_conditions.rs`

When reading these files on `bls-federation`, remember the current tree already contains later threshold/federation types. For the original precursor, inspect `git show bls:<path>`.

---

## Suggested presentation chapters so far

These four chapters cover `main..bls`. Federation should start after this.

### Chapter 1 — Why BLS before federation

Show the secp DHKE equations, then the pairing equations.
Point: we needed a signature scheme where wallets verify against one public key, but the secret can later be shared.
Stay explicit that original `bls` is still single-signer.

### Chapter 2 — Keyset v3 is the protocol switch

Show the naming trap, then the version matrix.
Show that v3 IDs are v2 IDs over G2 keys.
Show the mint default: new keysets are v3, live secp keysets stay until rotated.

### Chapter 3 — How a v3 proof is born and checked

Walk wallet derive → blind → mint sign → blind pairing check → unblind → proof pairing check.
Call out "no DLEQ on v3".
Call out the blind-signature verification fix.

### Chapter 4 — Mixed-curve types are a compatibility minefield

`PublicKey` length dispatch, P2PK/HTLC secp-only, rejection sampling, fuzzers.
This is the "we had to make the old SDK safe" chapter.

---

## Federation teaser, not yet explored

Do not present these as finished chapters yet. They are only breadcrumbs from the later `bls-federation` history:

- Federation mode is v3/BLS-only. v1/v2/non-BLS aggregate keysets are rejected.
- Threshold types were added on top of `nut01/bls.rs`.
- Later work includes dealer-free BLS DKG, federated keyset rotation, share verification, and wallet aggregation of member shares into ordinary Cashu proofs.
- Bitcoin custody (FROST / BDK / Bark) is a separate key hierarchy from Cashu BLS keysets.
- Consensus, Iroh transport, and Nocean multi-operator setup are later operational layers.

Next research pass should map federation the same way this file mapped BLS:

1. threshold BLS primitives
2. v3-only federation keyset gates
3. DKG and keyset rotation
4. wallet share aggregation
5. consensus / payments / transport

---

## Open questions / things to confirm before slides

- Is there a published NUT text for keyset v3 / BLS DHKE that we should quote, or are we presenting CDK's implementation ahead of the spec writeup?
- Which exact NUT-00 / nutshell test vectors should appear on slides?
- Do we want a live demo of a standalone v3 mint, or only equations plus code pointers?
- For the federation half, should FROST/Bark be a later appendix so the first talk stays on Cashu issuance?

---

## Alternative: many secp signatures can simulate a threshold

BLS threshold signatures are much more elegant, but they are not the only way to build a federated mint.

In principle the previous secp DHKE would have been enough. Each member would sign the same blinded message with its own mint key and return a complete standalone signature. The wallet would then keep a set of those signatures instead of one `C`. A `t`-of-`n` policy would be simulated the same way Bitcoin simulates a threshold script with bare multisig: the proof is valid iff it contains enough independently valid signatures from the advertised member keys.

That is a real federation. It just moves the threshold into the bearer object. The proof is no longer one signature under one key; it is a bundle of signatures. Later receive, swap, melt, and restore paths would all have to understand that bundle.

BLS keeps the threshold on the issuance side. Members still produce shares, but the wallet reconstructs one ordinary Cashu proof before the token exists.

The longer comparison below stays as working notes for why we did not take the naive path.

---

## Design fork: naive secp multisig vs BLS threshold

This is a presentation-worthy design question, not just a crypto preference.

Naive reading: "we already had Chaumian signatures, so a federation could just be t-of-n mint signatures, like Bitcoin bare multisig."

That would have worked as a prototype. It would have been a worse Cashu federation.

### Two different secp alternatives

Do not collapse these. They are not the same design.

1. Bare multisig (what the question usually means)
   - Each member has its own secp mint key `k_i`.
   - Wallet blinds once: `B_ = Y + rG`.
   - Each member returns a full signature `C_i = k_i · B_`, plus DLEQ if you want public verifiability.
   - A spendable proof carries t signatures, or the mint later demands t signatures.
   - Token format, verify, swap, melt, restore, and wallet APIs all become federation-aware.

2. Threshold / interpolated secp DHKE (the closer analogue to what BLS does)
   - Secret-share one secp key: `k` split into `k_i`.
   - Each member returns a share `C_i = k_i · B_`.
   - Wallet Lagrange-interpolates `C = Σ λ_i C_i`.
   - The result can be one ordinary secp Cashu proof under `K = kG`.
   - This is n-of-n if you only add shares, t-of-n if you Shamir-share `k`.

BLS is option 2 on a pairing curve, not option 1 with nicer bytes.

### Why bare multisig is the wrong Cashu federation

It breaks the product constraint that shows up everywhere in the federation docs: wallets should aggregate member shares into ordinary Cashu proofs, and the public wallet-to-mint protocol should stay Cashu.

Bare multisig fails that in concrete ways:

- A proof is no longer `(amount, keyset, secret, C)`. It is a roster plus t signatures.
- Vanilla wallets cannot receive or swap those tokens. The federation leaks into every later hop.
- Token size and verify cost grow with t. 3-of-5 is annoying. 7-of-10 is ugly.
- Keyset IDs have to commit to a set of keys, not one key. Rotation and old-proof verify become roster history, not "keep the old keyset around".
- NUT-12 DLEQ multiplies: one per signer, or you give up offline proof that each member used the advertised key.
- P2PK/HTLC stay secp, but the mint signature is the bearer instrument. Making *that* multisig makes the money itself federation-shaped.
- You still need consensus. Multisig does not stop two honest-looking subsets from signing two different output sets for the same quote. That mix-and-match attack is an ordering problem, not a signature-algorithm problem.

Bitcoin bare multisig is a good analogy for exactly this failure mode. Everyone can see the federation, every spender must know the policy, and the coin is never just "a signature under one key".

### Why interpolated secp DHKE is still worse than BLS

If the goal is "one final proof under one key", secp can do the algebra. EC addition interpolates. That is not the hard part.

The hard part is proving that the interpolated point is actually `k · Y` for the advertised aggregate key, without anyone knowing `k`.

Standalone Cashu solves that with DLEQ. DLEQ is a secp discrete-log-equality proof that uses the mint secret. In a federation nobody has `k`, so you need one of:

- per-share DLEQ against each published `K_i`, then hope interpolation is done honestly;
- a threshold / aggregated DLEQ, which is a new protocol;
- or silently trust the shares, which lets a malicious threshold produce a `C` that is not a signature under `K`.

BLS makes the same check work for a share and for the aggregate:

- share: `e(C_i, G2) == e(B, K_i)`
- proof: `e(C, G2) == e(H(secret), K)`

No extra DLEQ. No third-party verifier that has to understand the roster. After aggregation the token is a normal v3 proof.

That is why the original BLS branch forbids DLEQ on v3. Pairings replaced it.

Other reasons BLS wins for this specific system:

- Batch verification is already specified and implemented for v3.
- Share verification, blind-signature verification, and final-proof verification are one family of pairing checks.
- Keyset v3 already committed to one G2 key per amount. Federation can advertise an aggregate keyset that looks like a standalone v3 keyset.
- FROST is already used later for Bitcoin custody. Keeping Cashu issuance on BLS keeps the two key hierarchies from collapsing into one "threshold secp everywhere" soup.

### What naive multisig would still have gotten right

It is fair to say this, or the comparison sounds like marketing.

- Blindness still works. Members only see `B_`.
- t-of-n availability still works. Offline members can be skipped if the wallet can find t signatures.
- For a trusted 2-of-3 prototype among people who all run custom wallets, it would have been less crypto work up front.
- The expensive, necessary federation work is still consensus, replay, catch-up, payment observation, and "do not sign until the operation is accepted". That work does not disappear if you pick secp.

So the honest summary is:

- Naive secp multisig federates the *token*.
- BLS threshold federates the *issuer*, then emits a normal token.

Cashu cares about the second thing. The bearer object should stay boring.

### Presentation angle

Possible slide contrast:

| | Bare secp multisig | BLS threshold shares |
|---|---|---|
| What the wallet stores | t signatures + roster | one `C` under one `K` |
| Who can receive the token | federation-aware wallets | any v3 Cashu wallet |
| Verify | t secp checks + policy | one pairing, or a batch |
| Offline proof of mint key | per-signer DLEQ | pairing against advertised `K` |
| Keyset | set of keys | ordinary v3 keyset |
| Still need consensus? | yes | yes |

One-liner: Bitcoin can afford bare multisig because the coin lives in a UTXO that already names its script. Cashu cannot, because the coin *is* the signature.

### Open question for later

If someone asks "then why not FROST-sign the secret instead of BLS-DHKE?", that is a third design: replace Chaumian blinding with a threshold Schnorr signature. That is not Cashu anymore. It is a different eCash scheme. Worth a footnote, not the main fork.

---

## BLS threshold scheme we actually use

Short version: this is classic Shamir-threshold BLS, applied to Cashu's blinded G1 message. It is not "many independent BLS signatures glued together", and it is not the IETF aggregate-BLS construction that compresses signatures on different messages. One secret mint key is split. Any `t` members can reconstruct one ordinary v3 blind signature. Fewer than `t` learn nothing useful about that key.

The code lives in `crates/cashu/src/nuts/nut01/bls.rs`. Wallet aggregation is `aggregate_blind_signature_shares()`. Member signing is `BlsSecretKeyShare::blind_sign_share()`.

### What is shared

For each denomination, there is one secret scalar `k` in the BLS12-381 field `Fr`. That is the same `k` a standalone v3 mint would have used.

Pick a polynomial of degree `t-1`:

```
f(x) = a0 + a1 x + a2 x^2 + ... + a_{t-1} x^{t-1}
```

with `a0 = k`. Member `i` gets the evaluation

```
k_i = f(i)
K_i = k_i · G2
K   = k · G2 = f(0) · G2
```

Signer IDs are non-zero `u16` values. Zero is forbidden because interpolation is at `x = 0`, and `f(0)` is the secret.

The public object a wallet sees is still one G2 key `K` per amount. That is the v3 keyset. The `K_i` values are federation metadata, not part of the final token.

Two ways to create the same shares:

- Trusted dealer: one process picks the coefficients and hands `k_i` to each member. Used in tests.
- Dealer-free Pedersen DKG: every member picks its own polynomial, publishes G2 commitments, and privately sends evaluations to the others. Member `i` sums the incoming evaluations to get its final `k_i`. The joint secret is the sum of the `a0` values. Same math afterwards.

### Signing a blinded message

The wallet still blinds exactly as in standalone v3:

```
B = r · H(secret)     (G1)
```

Member `i` does not sign the secret. It multiplies the blinded point by its share:

```
C_i = k_i · B
```

Anyone can check that share against the published `K_i` without knowing `k_i`:

```
e(C_i, G2) == e(B, K_i)
```

That is the same pairing check as a standalone blind signature, just against the member key instead of the aggregate key.

### Combining any t shares

Given a set S of `t` distinct signer IDs, the wallet computes Lagrange coefficients at zero:

```
λ_i(0) = Π_{j in S, j ≠ i}  (0 - j) / (i - j)
       = Π_{j in S, j ≠ i}  (-j) / (i - j)
```

and interpolates in G1:

```
C = Σ_{i in S}  λ_i(0) · C_i
```

Because `C_i = f(i) · B`, this is just polynomial interpolation in the exponent:

```
C = f(0) · B = k · B
```

Any other valid subset of size `t` produces the same `C`. Order does not matter. Fewer than `t` shares are rejected. Duplicate signer IDs are rejected.

The public shares interpolate the same way in G2:

```
K = Σ_{i in S}  λ_i(0) · K_i
```

After aggregation, verification and unblinding are standalone v3:

```
e(C, G2) == e(B, K)
proof   = r^{-1} · C
e(proof, G2) == e(H(secret), K)
```

A 1-of-1 keyset is exactly a normal single-signer BLS key. The tests assert that.

### Tiny 2-of-3 example

Ignore the real field and pretend we can use small integers. Threshold 2 means a line:

```
f(x) = 7 + 3x
k = f(0) = 7
```

Members:

```
id 1:  k1 = 10
id 2:  k2 = 13
id 3:  k3 = 16
```

Wallet blinds a secret to some point `B`. The three possible shares are `10B`, `13B`, `16B`.

Members 1 and 2 combine:

```
λ1 = (0-2)/(1-2) = 2
λ2 = (0-1)/(2-1) = -1
C  = 2·(10B) + (-1)·(13B) = 7B
```

Members 2 and 3 combine:

```
λ2 = (0-3)/(2-3) = 3
λ3 = (0-2)/(3-2) = -2
C  = 3·(13B) + (-2)·(16B) = 7B
```

Same blind signature. Member 1 alone has `10B`, which is not `7B`. The wallet then unblinds `7B` with `r` and holds a normal v3 proof under `K = 7·G2`.

The in-tree 3-of-5 test does this with real BLS scalars: subsets `{1,3,5}` and `{2,3,4}` both equal the dealer secret times `B`.

### What this is not

- Not bare multisig. The token does not keep `C_1 ... C_t`. Those shares exist only until the wallet interpolates `C`.
- Not FROST. FROST is the later Bitcoin-custody threshold scheme on secp256k1. Different curve, different key, different job.
- Not "BLS aggregation" of different messages. Every share here is a share of the *same* `k · B`.

### Why it fits Cashu

Because the combination happens on the blinded point, the mint members never see the secret, and because the combination produces `k · B`, the rest of Cashu can stay dumb: one keyset, one `C`, one pairing check. The threshold lives in issuance, not in the bearer token.

---

## Threshold-signing mix-and-match

This is the attack the federation docs keep circling. Informal talk often calls it the **sliding-window attack**. The better name already in the docs is **threshold-signing mix-and-match**.

Authoritative wording, from `docs/federated-cashu-rfc.md`:

> The motivating attack is threshold-signing mix-and-match: a wallet presents different outputs or requests to different members and combines overlapping share subsets.

Other names in the same corpus:

- `docs/federated-cashu-formal-review.md`: **overlapping-window mix-and-match**
- `docs/federated-cashu-implementation-plan-completed.md`: **sliding-window quorum-cover attack**, plus Milestone 12 "explicit sliding-window attack regression suite"
- `docs/federated-cashu-consensus-current.md`: **threshold-signature mix-and-match**, illustrated with the ABC / BCD / CDA / DAB windows
- Tests: `runtime_config_rejects_sliding_window_mint_output_attack`, `local_federated_members_reject_sliding_window_swap_output_attack`

Use **threshold-signing mix-and-match** as the name. Use **sliding window** for the overlapping-subset picture.

### Why this attack exists

A BLS share is independently valid. `C_i = k_i · B` checks against member `i`'s public share even if nobody else signed that same `B`.

If members sign whatever blinded outputs they are shown, a wallet can:

1. pay once, or present one input set once;
2. show member A one output set and member B a different output set;
3. collect `t` shares for *each* output by rotating which members see which window;
4. interpolate more finished proofs than the quote or inputs authorized.

Consensus is what stops it. Members must sign only the one canonical operation that won ordering. Same quote or same inputs plus a different output set is a conflict, not a second signing job.

This is not a BLS-math bug. It is what happens when a threshold issuer signs before it agrees on the exact message.

### Simple 3-of-5 example

Federation members `M1 … M5`. Signature threshold `t = 3`. The wallet paid a quote that authorizes **2** tokens.

Honest minting would create two outputs, `A` and `B`. The attacker prepares three outputs, `A`, `B`, and `C`.

First presentation:

- `M1, M2, M3` are asked to sign `{A, B}`
- `M4, M5` are asked to sign `{B, C}`

Locally both requests look fine: two outputs for a two-token quote. After this pass:

| Output | Shares | Enough? |
|---|---|---|
| A | M1, M2, M3 | yes |
| B | M1 … M5 | yes |
| C | M4, M5 | no, only 2 |

Second presentation, slide the window:

- `M3, M4, M5` are asked to sign `{B, C}`

If members sign before comparing notes, `M3` may sign the second set too. Now:

| Output | Shares | Enough? |
|---|---|---|
| A | M1, M2, M3 | yes |
| B | everyone | yes |
| C | M3, M4, M5 | yes |

Paid 2, minted 3. The extra token is `C`.

ASCII picture:

```
Paid quote: 2

           A              B              C
M1         *              *
M2         *              *
M3         *              *              *   ← saw both windows
M4                        *              *
M5                        *              *

Each column has 3 stars. Each output aggregates.
```

The same trick works on swap: keep the inputs fixed, slide the output set. The inputs get spent once in each member's local view, but the wallet reconstructs extra change or extra swap outputs from the overlapping shares.

### The sliding-window picture the tests use

Milestone 12 uses four overlapping 3-output windows against one paid 3-token quote. That is the literal "window" that slides:

```
Outputs: A B C D     (4 notes)
Paid for: 3

Window ABC → some members
Window BCD → the next members
Window CDA → the next
Window DAB → the next
```

Each member only ever sees 3 outputs, so each request looks balanced. Across the roster, every output appears in enough windows to gather 3 shares. The unpaid fourth note becomes spendable.

A 3-of-5 assignment that makes the cover obvious:

```
M1 signs A B C
M2 signs B C D
M3 signs C D A
M4 signs D A B
M5 signs A B C

A: M1, M3, M4, M5
B: M1, M2, M4, M5
C: M1, M2, M3, M5
D: M2, M3, M4
```

Four finished proofs from a 3-token payment.

### What the implementation does about it

- Hash the exact envelope into a `FederationOperationId`. Same bytes, same ID. Different output set, different ID.
- Conflict-key the quote (mint) or the inputs (swap/melt). Second output set for the same quote/inputs is a conflict, not a new operation.
- Sign only after that exact operation is accepted. Shares carry the accepted `operation_id`.
- Naive "each member signs what it was shown" is kept only as a test double that the sliding-window suite is supposed to beat.

If members signed first and ordered later, a signing threshold below the consensus threshold would bring the attack back. That is why the formal review says a 3-of-5 signing threshold is safe only because shares are produced after the exact operation is accepted.

---

## Consensus before signing

Threshold BLS makes any `t` valid shares enough to finish a proof. Mix-and-match is what happens if those shares are for *different* output sets. Consensus is what puts the members back on one output set before anyone signs.

Treat AlephBFT as a black box. Members put envelopes in. The box eventually emits one total order: first this exact swap, then that exact mint, and so on. A member does not sign because it received an HTTP request. It signs because that exact envelope came out of the box as accepted.

### A swap is a named envelope

A federated swap is not "please sign these outputs." It is:

```
envelope = {
  federation,
  kind: Swap,
  inputs:  [P1, P2],
  outputs: [A, B]
}
operation_id = hash(envelope)
```

Same bytes, same ID. Change one output, different ID. That ID is the name of the proposed transaction.

The public swap path is:

1. wrap the ordinary Cashu `SwapRequest` as that envelope;
2. submit it to the consensus box;
3. wait until that exact ID is accepted;
4. only then spend the inputs and return BLS shares for those outputs.

Apply does the Cashu part: verify the proofs, mark the inputs spent, sign the accepted outputs. Replay of the same accepted swap is idempotent. A later different output set for already-spent inputs returns `TokenAlreadySpent` and writes no shares.

### How this kills the sliding window

3-of-5 members. Wallet holds two proofs `P1 P2`. Honest outputs are `{A, B}`. Attacker also prepares `{B, C}`.

Without consensus, members can sign different windows:

```
M1 M2 M3 see {A, B}  → shares for A and B
M4 M5    see {B, C}  → shares for B and C
then M3 also sees {B, C}
A and C both reach 3 shares
```

With consensus those are two different envelopes:

```
S_AB = hash(inputs=P1,P2  outputs=A,B)
S_BC = hash(inputs=P1,P2  outputs=B,C)
```

The box can accept only one first. Suppose `S_AB` wins. Every honest member then applies the same story:

```
P1, P2  → spent
A, B    → sign
C       → not in the accepted operation, so nobody signs it
```

When the attacker later shows `S_BC` to `M4` or even to `M3`, that member does not say "looks balanced, here is a share." It says "I will only sign if the box accepted `S_BC`." The box will not accept a second live swap of `P1 P2`, because those proofs are already spent in the accepted history.

```
wallet
  │  swap(P1,P2 → A,B)     swap(P1,P2 → B,C)
  ▼
member HTTP
  │  wrap as envelope
  ▼
consensus box
  │  one winner: S_AB
  ▼
every honest member
  spend P1,P2
  sign only A,B
```

The sliding-window swap test does the same thing with more windows. It first accepts `{A,B,C}` for a fixed input set, then tries `{D,E,F}`, `{B,C,D}`, `{C,D,A}`, and `{D,A,B}`. The later windows die with `TokenAlreadySpent`. `D E F` never get shares.

The useful picture is not "members gossip about signatures they already made." They agree on the transaction *before* any signature exists.

### Mint vs swap lock

Mint and melt conflict on the quote ID. Two different output sets for the same paid quote are the same conflict key and different operation IDs, so the second is rejected as a conflict.

Swap currently does not use a quote-style conflict key on the input proofs. The inputs themselves become the lock once the first accepted swap spends them. Same result for mix-and-match: the second window cannot collect shares.

### Why signing threshold can be below consensus threshold

A 3-of-5 signing threshold with 4-of-5 BFT is safe only because shares are produced after the exact operation is accepted. If members signed first and ordered later, the lower signing threshold would bring mix-and-match back. Consensus is the agreement. BLS is only the compact way to emit the already-agreed signature.

---

## Data flow and transport

There are two networks in a federation, not one. Same members, two jobs, two topologies.

The public plane is wallet to mint. The private plane is member to member. HTTP and Iroh are just two ways to carry those same two planes. `iroh://...` is not a new protocol. It is the same HTTP-shaped Cashu and `/federation/v1` traffic, tunneled over Iroh instead of TCP.

### Public plane: wallet fan-out

A normal Cashu wallet talks to one mint URL. A federated wallet still looks like that to the rest of CDK. `FederatedMintConnector` sits underneath and fans the request out.

Each member advertises a `public_mint_url`. For signing work — mint, swap, melt, restore — the wallet sends the *same* ordinary Cashu request to every member. Each member independently wraps that request, submits it to consensus, and, if that exact operation is accepted, returns *its* BLS share. The wallet waits until it has a threshold of matching shares, checks each share against that member's published `K_i`, interpolates one `C`, and hands an ordinary Cashu response back to the saga.

```
            mint / swap / melt
wallet ──────────────────────────► M1  ── share1
    │                             M2  ── share2
    │                             M3  ── share3
    │                             M4  ── timeout / offline
    └───────────────────────────► M5  ── share5

wallet keeps any 3 valid shares → one proof
```

That is a star, not a gossip mesh. The wallet is the hub. Members do not send shares to each other. The wallet is the aggregator.

It does not wait for everyone. Default fan-out starts all members, but liveness is the signature threshold. A slow minority can be ignored. Offline members are fine. Bad shares are dropped. Two members returning shares for different output sets cannot be mixed; they are different operations.

Quote *creation* is the exception. The wallet does not need `t` invoice quotes. It tries members one at a time until one healthy member accepts, then checks that the quote becomes visible to a threshold before returning it. Status polls fan out again, because the wallet must not trust one member's "paid" or "issued."

So:

- create quote: first healthy member, then threshold visibility
- sign / swap / melt / restore: full fan-out, threshold aggregation
- the token that leaves the wallet is still one Cashu proof

Code: `crates/cdk/src/wallet/federation.rs`.

### Private plane: members talking to members

Fan-out alone cannot stop mix-and-match. If M1 and M4 only hear from the wallet, they can still be shown different windows. They need a second network so they can agree *before* they sign.

Each member has a second URL, `federation_api_url`, mounted at `/federation/v1`. That traffic is authenticated with the member identity key. Wallets should not be on it.

That private plane carries:

- consensus: submit this envelope, here is an AlephBFT message, wake up, I am at journal height 40, send me entries 41–50
- DKG, later: commitments, reveals, secret-share delivery, transcript signatures, activation

```
wallet ★ fan-out ★ members          public Cashu URLs

M1 ── M2
│  ╲╱  │                           private /federation/v1
│  ╱╲  │                           consensus + DKG
M3 ── M4
   ╲  ╱
    M5
```

A swap then looks like this in time:

1. Wallet fans `swap(P1,P2 → A,B)` to everyone.
2. Each member that hears it submits the same hashed envelope into the private consensus plane.
3. The black box eventually says: `S_AB` is accepted.
4. Each ready member spends `P1 P2` and returns only shares for `A` and `B`.
5. Wallet aggregates those shares.

If the attacker also fans `{B,C}` to a subset, that is a different envelope. The private plane will not accept it as a second live swap of the same inputs. The public shares for `C` never appear.

### HTTP and Iroh

Both planes can run on either transport.

- HTTP: `https://mint-a.example` for wallets, `https://mint-a.example/federation/v1` for peers.
- Iroh: `iroh://<endpoint-id>` for the same routes. The Axum router is reused. Iroh is the pipe.

They can be mixed. Tests already dial a roster where some members are HTTP and some are Iroh from one transport. A wallet can fan out over Iroh; members can run consensus over Iroh; or only the private plane is Iroh and wallets stay on HTTPS.

The important split is not HTTP vs Iroh. It is public fan-out vs private mesh:

- public: wallet asks everyone, collects shares, emits a normal token
- private: members agree on the exact operation, and later run DKG
- transport: HTTP or Iroh, same messages

---

## BLS DKG

BLS DKG is how the federation makes the Shamir shares without a trusted dealer. Nobody ever holds the full mint key `k`. At the end, the wallet still sees one ordinary v3 keyset.

The code calls this dealer-free Pedersen DKG. The ceremony lives on the private member plane, not on the wallet fan-out. Trusted-dealer setup is the same end state and is used in tests. Production wants this ceremony so no one machine ever sees `k`.

### What they are generating

For threshold `t`, each amount needs a polynomial

```
f(x) = k + a1 x + ... + a_{t-1} x^{t-1}
```

Member `i` should end up with `k_i = f(i)`, and the advertised mint key should be `K = k · G2`. A trusted dealer could pick `f` and hand out the `k_i` values. DKG is the same result, except every member contributes a little secret and the joint `k` is the sum of those secrets.

Each member `j` picks its own polynomial `f_j`. The real mint secret is

```
f(x) = f_1(x) + f_2(x) + ... + f_n(x)
k    = f(0) = f_1(0) + ... + f_n(0)
```

Member `i` never sees anyone else's polynomial. It only receives the evaluation `f_j(i)` from each dealer `j`, checks that evaluation against `j`'s public commitments, then adds them:

```
k_i = f_1(i) + f_2(i) + ... + f_n(i)
```

That `k_i` is exactly the Shamir share used later to sign `C_i = k_i · B`.

They do this once per denomination, because a Cashu keyset is one key per amount. Same ceremony, many polynomials.

### The dance

They cannot show `f_j` itself. They publish the public version of its coefficients in G2:

```
C_j = [ f_j(0)·G2,  next coeff · G2,  ... ]
```

The public steps are commit, reveal, privately deliver, then add.

1. Agree what they are generating: roster, threshold, unit, amounts, fees, expiry. If those differ, the ceremony cannot finish.

2. Each member generates its polynomials locally, immediately evaluates a share for every receiver, then throws the raw coefficients away. What remains is a public reveal and a sealed envelope for each peer.

3. Hash-commit first. Every member publishes `H(my later reveal)` before anyone shows the actual G2 points. That stops the last member from peeking at everyone else's keys and then choosing a convenient polynomial.

4. Reveal. Everyone opens the G2 commitments for their coefficients. The first coefficient is that member's public piece of `K`. The rest let others check later evaluations. The reveal must match the earlier hash, or it is rejected.

5. Private delivery. Member `j` sends member `i` the values `f_j(i)` for each amount, over the authenticated private plane. Member `i` checks

```
f_j(i) · G2  ==  evaluate j's public polynomial at i
```

A lie is visible. A good share is kept.

6. Sum. Member `i` adds every verified contribution and gets its final `k_i`. Publicly, everyone adds the constant-term commitments and gets the aggregate mint key `K`. They also add the evaluated public shares to get each `K_i`. Those are exactly the v3 keyset plus federation metadata.

7. Transcript and activation. They hash the finished public ceremony, collect member signatures, and only then activate the keyset. Until that happens, nobody should sign eCash with it.

### How they arrive at the aggregate public key

The aggregate public key is the same addition as the secret shares, but only with the constant terms that were revealed in the open:

```
K = C_1[0] + C_2[0] + ... + C_n[0]
  = (f_1(0) + f_2(0) + ... + f_n(0)) · G2
  = k · G2
```

That `K` is what goes into the v3 keyset. Wallets never see the dance. They just see one mint public key. Each member also publishes `K_i = k_i · G2`, which is how a later signature share can be checked without reconstructing `k`.

So: hash so nobody can adapt, reveal so everyone can check, privately send evaluations, add. The public key is the sum of the opened constant terms. The secret `k` never exists in one place.

### Tiny 3-member, 2-of-3 picture

```
M1 picks f1(x) = 4 + 1x
M2 picks f2(x) = 2 + 5x
M3 picks f3(x) = 1 + 2x

joint f(x) = 7 + 8x
joint k    = 7
```

What M2 actually receives:

```
from M1: f1(2) = 6
from M2: f2(2) = 12
from M3: f3(2) = 5
sum           = 23 = f(2)
```

M2 can check each incoming number against that sender's published commitments. It never learns `7`. Later, any two members can reconstruct signatures under `K = 7 · G2`.

### Why this belongs after fan-out and consensus

- DKG uses the private member mesh, not wallet fan-out.
- After DKG, issuance is still: wallet fans out, members agree on the exact operation, then each uses its `k_i`.
- Trusted-dealer setup is the same end state, used in tests. Production wants this ceremony so no one machine ever sees `k`.

Code: `crates/cdk-common/src/federation/dkg.rs`, plus the share helpers in `crates/cashu/src/nuts/nut01/bls.rs`.

---

## Challenges: missing SIG_ALL / client spend intent

Consensus stops mix-and-match among honest members. It does not stop one Byzantine member from spending the wallet's proofs first.

This is still open. The implementation plan says the federation is not production-ready because bearer proofs do not authenticate client intent against a Byzantine receiving member. The audit name is `SEC-2026-07-17-01`. Cleanup work was explicitly told not to claim it fixed this.

### The hole

A normal Cashu proof is a bearer instrument. Whoever can present `(secret, C)` can spend it. Federated swap currently works like this:

1. Wallet fans the full `SwapRequest` to every member, including the raw input proofs.
2. The receiving member wraps that request in a federation envelope and authorizes it with the *member* identity key.
3. Consensus checks that a roster member signed the envelope. It does not check that the proof owner signed those exact outputs.

So the operation is authorized as "a federation member said this swap should happen," not "the owner of these proofs wanted these outputs."

For an ordinary bearer proof, seeing the request is enough to build another valid spend. A Byzantine member can:

1. receive the wallet's swap `inputs = [P1, P2], outputs = [A, B]`;
2. copy the valid input proofs;
3. replace the outputs with its own blinded messages `[X, Y]`;
4. submit that replacement as a legitimate member-authored operation;
5. race it into the consensus box;
6. if the replacement is accepted first, honest members spend `P1 P2` and sign `X, Y`;
7. the wallet's original `{A, B}` then fails with `TokenAlreadySpent`.

The database still prevents two committed spends. The attacker spend wins. The user spend loses.

```
wallet fans   P1,P2 → A,B     to M1..M5

evil M5 copies P1,P2
         builds P1,P2 → X,Y
         submits that envelope first

consensus box
   winner: P1,P2 → X,Y

honest members
   spend P1,P2
   sign X,Y   ← attacker outputs

wallet
   P1,P2 → A,B is now spent-against
```

This is not the sliding-window attack. Sliding-window needs different members to sign different honest-looking windows. This needs only one evil member who saw the proofs, plus the ability to get its rewritten swap ordered first. Consensus will happily accept the rewritten swap if it is a balanced, well-formed operation. Honest members cannot tell that `X, Y` were not the user's outputs.

The same shape exists for melts if a member can reuse revealed bearer proofs against another quote. Quote conflict keys are not client authorization.

### Why SIG_ALL is the Cashu-shaped fix

NUT-11 already has the tool: lock the proof to P2PK and sign with `SIG_ALL`. That witness commits the owner to the exact outputs of the spend. Changing `A, B` to `X, Y` invalidates the signature.

Then admission and apply can reject the rewritten swap before it becomes the accepted operation. The evil member still sees the proofs, but it can no longer produce a *different* valid spend.

A detached key that was not committed when the proof was minted does not help. The owner has to bind the spend at proof-creation time, or at least with a secret that the receiving member does not already have from the request itself.

If ordinary unlocked bearer proofs stay supported, one receiving member remains trusted not to steal them. That is not Byzantine client-fund safety.

### What this is not

- Not a BLS bug. The shares are valid. They are for the wrong outputs.
- Not mix-and-match. Only one operation wins; it is the attacker's.
- Not solved by fan-out. Fan-out is how the evil member learns the proofs.
- Not solved by DKG. DKG protects `k`, not the user's bearer secrets.

### Status

Documented as critical in `docs/federated-cashu-security-audit-2026-07-17.md`. Still listed as a production blocker in the implementation plan. The current code can bind SIG_ALL when the user already used it, and tests prove a SIG_ALL output mutation changes the operation ID and fails apply. It does not *require* SIG_ALL on every federated spend. Until that protocol decision is made, the federation should not be described as safe for users against one malicious member.

---

## Funding backends: the other half of the mint

A Cashu mint is two machines glued together.

One machine is the eCash issuer: it signs proofs. That is the BLS federation. The other machine is the funding backend: Lightning or on-chain. That is where the real money sits when somebody mints, and where real money leaves when somebody melts.

If you only federate the first machine, you have not built a federation. You have built a committee that prints IOUs against a piggy bank that one person still holds.

### Why classical connectors are not enough

Standalone CDK talks to one CLN, one LND, one BDK wallet, or one Bark wallet. Those connectors assume one process has the keys, sees the invoices, and can pay.

Point every federation member at one such node and that node's operator can:

- mark a quote paid when nothing arrived, and the BLS side will issue eCash;
- refuse to pay a melt, or pay a different invoice;
- sweep the on-chain or Lightning funds while the eCash ledger still looks honest.

The other members can vote all they want. They cannot stop a key they do not share.

So the funding backend has to give every member the same grade of access. Same view of deposits and invoices. Same ability to refuse a spend. Same inability to spend alone. The quorum that can issue eCash must be the quorum that can move the reserves. If those two quorums diverge, the smaller one is the real mint.

### What we built instead

Two federated backends replace the single-operator connectors:

- **Federated BDK** is the on-chain treasury. Members do not load a mnemonic. They watch the chain, agree on addresses and unsigned transactions through the same consensus box, then jointly sign those transactions. One member can propose "send these coins to this melt address." It cannot actually move them.
- **Federated Bark** is the Lightning treasury. Incoming mint invoices and outgoing melt payments are planned as consensus operations first. Claiming an incoming HTLC or sending an outgoing payment is a threshold custody action, not "member 3's CLN paid it." One member can see a payment and report it. It cannot unilaterally take or release the funds.

Both backends use FROST on secp256k1. That is a different key, on a different curve, for a different job. BLS is for blinded Cashu signatures. FROST is for Bitcoin / Lightning signatures the outside world already understands. Do not mix the secrets. Setup is only complete when both ceremonies exist: BLS for the eCash keyset, FROST for the treasury. The FROST DKG is a later section.

### Why the same quorum as eCash consensus

Mint and melt are one economic act with two sides.

Mint: money in, proofs out. If three members can issue proofs but one member can invent a "paid" observation, eCash inflates. If three members must agree the quote is paid, but one member can later sweep the on-chain deposit, the remaining proofs are unbacked.

Melt: proofs in, money out. If three members can burn the proofs, but one member can redirect the payout, users get robbed. If three members must sign the payout, but one member can still issue change or reopen the proofs, the ledger and the treasury disagree.

The safe rule is blunt:

- the same roster
- the same consensus history
- a signing threshold that cannot be weaker than the eCash threshold in a way that lets a minority move funds or invent deposits

Fakewallet and `development_single_observation` exist for tests. They explicitly trust one local observation. That is not a federation funding model. A production policy has to require a member quorum, *and* the backend itself has to be threshold-held. Quorum observations against a single-key CLN still leave one trusted operator.

Only a backend that guarantees shared access is safe to attach to this eCash federation. Classical single-mint connectors are fine for standalone CDK. In a federation they reintroduce a king.

---

## FROST DKG

Federation startup runs two independent dealer-free ceremonies under the same roster, ceremony ID, and `signature_threshold`:

- BLS12-381 DKG → Cashu eCash keysets
- secp256k1 FROST DKG → one application-neutral Bitcoin / Lightning root

Both must finish. A partial result never makes the mint ready. The secrets must not be mixed. BLS blinds and signs Cashu. FROST signs Bitcoin and Lightning the outside world already understands.

### High level

FROST is threshold Schnorr. After DKG, the federation has one secp256k1 group public key `P`. Each member holds a share `s_i` such that any `t` members can later produce a normal BIP340 Schnorr signature under `P`. Nobody ever reconstructs the private key.

That root is not a BDK wallet and not a Bark invoice key. It is an untweaked, application-neutral package. Later, members derive on-chain and Lightning keys from it. The DKG itself only creates the root.

Same economic rule as the funding-backend section: the people who can issue eCash must be the people who can move the reserves. FROST uses the same `signature_threshold` as BLS on purpose.

### The ceremony

Library: `frost-secp256k1-tr` 3.0.0, isolated in `cdk-frost`. Ciphersuite `secp256k1_sha256_tr_v1`. Protocol version 2. Member IDs map 1:1 onto FROST identifiers.

Every FROST message binds federation ID, ceremony ID, setup authorization, monotonic root epoch, protocol, ciphersuite, threshold, ordered roster, sender, and receiver where applicable. BLS and FROST transcripts cannot be confused.

Three rounds on the private member plane:

1. **Round one, broadcast.** Each member publishes a FROST package: polynomial commitments and proofs. This is the public "I have a secret polynomial" step. Distinct signed REST kind: `/dkg/frost/round1`.

2. **Round two, confidential.** Each member sends every other member a per-recipient contribution. Only the intended receiver can use it. Distinct signed REST kind: `/dkg/frost/round2`. Wrong recipient, wrong ceremony, or replay from another context is rejected.

3. **Root confirmation.** Each member converts the result to the canonical untweaked root, checks its local share against its public verification share, and publishes the exact public bytes. Activation requires every configured member to confirm the same public package, plus a complete BLS result. Mismatch aborts setup and leaves the old config untouched.

Persist-before-send: sealed local phase state is written before any dependent outbound package. Exact replay is idempotent. A second different payload for the same ceremony/kind/sender/receiver is equivocation. A phase does not advance until the complete outbound batch is acknowledged, so a partition cannot leave one member holding a package another member never got.

At rest, the local signing share is AES-256-GCM ciphertext. Public records never contain a plaintext share. Associated data binds ciphertext to phase, member, ceremony, sender/receiver, and the public root package.

### What comes out

Public root package:

- root epoch
- protocol and ciphersuite
- threshold
- ordered member → verification-share map
- full compressed group key `P`
- transcript binding

Local package, sealed: only that member's signing share `s_i`. No mnemonic, no BDK descriptor, no Bitcoin network, no child index, no Taproot tweak, no BLS material.

Applications derive later, in this exact order:

1. canonical untweaked root
2. application-domain additive tweak
3. domain-separated public chain code
4. non-hardened BIP32 branch and child
5. BIP340 even-Y normalization during FROST signing
6. BIP341 key-path-only Taproot tweak for on-chain

Bark and BDK are consumers of this root. They do not run a second DKG for the treasury key.

### Slide bones

```
setup
  ├─ BLS DKG   → k_i, K     Cashu proofs
  └─ FROST DKG → s_i, P     Bitcoin / Lightning

FROST rounds
  1. broadcast commitments
  2. private per-recipient shares
  3. all members confirm the same P

then
  any t members → ordinary Schnorr under P
  never reconstruct the private key
  same roster + same threshold as BLS
```

One-liner: BLS DKG makes the eCash mint key; FROST DKG makes the treasury key; startup is not done until both exist and every member agrees on both public results.

Code: `crates/cdk-frost`, `crates/cdk-common/src/federation/frost_dkg.rs`, driver in `crates/cdk-axum/src/federation/dkg_driver.rs`.

---

## BDK mint and melt, step by step

Two public Cashu calls still look like a normal mint. Almost everything that makes federated BDK safe is hidden behind them, on the private plane.

Wallet APIs stay Cashu. Federation APIs are private: submit envelope, peer status, catch-up, chain observations, address allocation, transaction proposal, FROST rounds, signed-tx, broadcast intent. HTTP or Iroh, same messages.

```
wallet  --public NUT--  any member
                         |
                         v
                   consensus box
                    /         \
            BLS eCash          BDK/FROST coins
         shares to wallet     tx to Bitcoin
```

### Mint

1. Wallet `POST` mint quote, on-chain method, amount. Connector tries first healthy member, not full fan-out.

2. That member does not call BDK `reveal_next_address()`. It submits two consensus operations: `MintQuote` and `AllocateAddress`. The allocation binds quote, application epoch, external keychain, and the next unused derivation index. Replay can only reveal that exact index.

3. After accept, every honest member derives the same address from the FROST/BIP32 descriptor. The quote response the wallet sees is ordinary Cashu: quote id + Bitcoin address.

4. Wallet then fans quote-status until a threshold can see that same quote. One member's invoice is not enough.

5. User pays that address. Members' watch-only BDK wallets see a UTXO. That is a cache, not truth.

6. Each member submits a deposit / chain observation: exact outpoint, amount, script, block, confirmation depth, allocation. A minority cannot mark money received. Need the configured observation quorum and confirmation depth.

7. Only after that do `MintQuotePayment` observations finalize `amount_paid`. Matching observers must agree on quote, method, state, amounts, and payment identity. One "I saw it" does not issue eCash. Two incompatible quorums leave the quote unpaid.

8. Wallet polls status (fan-out). When paid, it `POST /mint` with blinded outputs to every member.

9. Each member wraps the exact outputs as a `Mint` envelope, submits it, waits for accept, then returns a BLS share. Conflict key is the quote: a second output set is rejected. Wallet verifies shares against `K_i` and interpolates one v3 proof.

Checks that matter:

- address is consensus-owned;
- mempool is not payment;
- reorg before issuance unsays the UTXO;
- reorg after issuance raises an alarm and does not unmint;
- issuance still requires the paid quote and the exact output set.

### Melt

1. Wallet `POST` melt quote with a destination Bitcoin address. First healthy member, then threshold visibility.

2. Wallet `POST /melt` with proofs and optional change, fan-out. Members submit a `Melt` envelope. Accept means: proofs reserved/spent, change outputs bound, and an on-chain payment intent created. `make_payment` does not send coins.

3. Some member may build a candidate tx in watch-only BDK. Consensus does not take a PSBT on faith. `TransactionProposal` carries the exact unsigned transaction: inputs, prevouts, destination, amount, fee, weight, sighashes, optional change to a consensus-allocated internal address.

4. Every member recomputes ownership, value conservation, fee caps, destinations, and Taproot sighashes. Conflict keys lock the batch, each UTXO, each intent. One owner. Sticky reservations: even if broadcast is ambiguous, those coins stay reserved.

5. After the proposal is accepted, FROST starts. Per input: authorization binds operation + tx + input index + exact 32-byte sighash. Round one commitments, then shares. Coordinator only aggregates. Each signer rebuilds the sighash and refuses a wrong message, epoch, or signer set. Nonces go `vacant → reserved → published → consumed`; failures burn, they do not reuse.

6. `TransactionSigned` persists the exact signed bytes before anyone talks to Bitcoin. Then a `BroadcastIntent` for those bytes only. Any ready member may broadcast. Observations cannot swap in another txid.

7. Confirmation observations, again to quorum and depth. Then `MeltQuotePayment` can mark paid. Wallet's melt status / change path fans out; change shares aggregate like mint shares.

Checks that matter:

- one member cannot redirect the payout;
- cannot sign a different tx;
- cannot broadcast unsigned or alternate bytes;
- cannot spend a UTXO that is not accepted-and-spendable;
- change, if any, is BLS and still only for the accepted melt.

---

## Swap, step by step

A federated swap never touches Bitcoin. No quote, no BDK, no FROST. It is only eCash: spend old proofs, issue new ones. The wallet API is still ordinary `POST /swap`. Everything that makes it safe is the same consensus-before-sign rule as mint, without a funding backend.

```
wallet  --public POST /swap--  every member
                                |
                                v
                          consensus box
                                |
                    spend P1,P2 and BLS-sign A,B
                                |
wallet aggregates shares --> ordinary new proofs
```

### Messages

Public, wallet to each member:

- `POST /swap` with a normal Cashu `SwapRequest`: input proofs, witnesses if any, blinded outputs.
- Response looks like a normal `SwapResponse`, but in federation mode the "signatures" are that member's BLS shares, not a finished mint signature.

Private, member to member, same `/federation/v1` plane as mint:

- `SubmitOperation` of a `Swap` envelope
- AlephBFT activity / network messages
- peer status and journal catch-up

No address allocation, no chain observation, no transaction proposal, no FROST rounds.

### Steps

1. Wallet already holds proofs `P1, P2` and blinds replacement outputs `A, B`. The federation connector validates those outputs against the advertised v3 keysets, then fans the *same* `SwapRequest` to every member's `public_mint_url`.

2. Each member that is ready for public submit wraps the request as:

   ```
   envelope = { federation, kind: Swap, request: {inputs, witnesses, outputs} }
   operation_id = hash(envelope)
   ```

   Same bytes, same ID. Change one output or one witness, different ID. Admission rejects empty inputs, duplicate inputs/outputs, non-v3 outputs, oversized proof/witness content, and unbalanced amounts before the envelope is proposed.

3. The member does **not** sign yet. It calls `submit_federation_envelope`: check local readiness / catch-up, then put the envelope into the consensus box. Private traffic is member-authenticated. Lagging members fail closed and return no shares.

4. Other members hear the same envelope through AlephBFT, or they hear it because the wallet also posted the same request to them and they submitted the identical `operation_id`. Exact retries are idempotent. A different output set is a different operation.

5. The box eventually accepts one winner. Suppose `S_AB = hash(P1,P2 → A,B)` wins.

6. Every honest member applies that exact journal entry:

   - verify input proofs against the aggregate v3 keys, including P2PK/HTLC/SIG_ALL if present;
   - verify outputs are v3/BLS and the transaction balances after fees;
   - mark `P1, P2` spent;
   - persist blinded messages `A, B`;
   - produce local BLS shares `C_i = k_i · B` only for those accepted outputs;
   - bind each share to `operation_id`, member ID, keyset, amount, output index.

   Replay of the same accepted swap is idempotent: shares are returned again, inputs stay spent. A later different swap of the same inputs dies with `TokenAlreadySpent` and writes no shares.

7. The public handler then returns `persisted_swap_share_response`: this member's shares for `S_AB`, never a standalone final signature.

8. The wallet collects member responses. It does not wait for everyone. It needs a signature-threshold of shares for the *same* outputs. Each share is checked with `e(C_i, G2) == e(B, K_i)`. Then Lagrange interpolation yields one `C` per output, unblinded into ordinary v3 proofs.

If a subset of members instead saw `P1,P2 → B,C`, that is `S_BC`, a different envelope. After `S_AB` is accepted, `S_BC` cannot collect shares. That is the sliding-window defense.

### Guarantees

What honest consensus gives:

- only one exact input/output set can be signed;
- inputs are spent once, across the roster;
- shares exist only after accept, and only for the accepted outputs;
- restore later can return those same shares, never new ones;
- offline members can catch up and still serve the accepted shares, they cannot invent another window.

What it does **not** give, unless the proofs already use SIG_ALL:

- a Byzantine member who saw the bearer proofs cannot be stopped from rewriting `P1,P2 → X,Y` and racing that envelope first.

If SIG_ALL is present, the witness commits to the exact outputs. Mutating them changes the operation ID *and* fails apply. That is why SIG_ALL is the remaining client-intent hole, not a missing swap step.

### Contrast with BDK mint/melt

| | Swap | BDK mint / melt |
|---|---|---|
| Public API | one `POST /swap` | quote, then mint or melt |
| Fan-out | full, for shares | first-healthy quote; full fan-out for mint/melt |
| Consensus objects | one `Swap` envelope | quote, address, observations, mint/melt, plus tx proposal / FROST |
| Lock | spent input proofs | quote ID, then UTXOs / payment intents |
| Funding backend | none | federated BDK + FROST |
| Wallet result | new eCash proofs | new eCash proofs, or Bitcoin left / received |

---

## Consensus messages on the wire

AlephBFT does not order raw HTTP. It orders `FederationConsensusItem`s. Almost every cash-relevant item is one wrapped envelope:

```
FederationConsensusItem
  Operation(FederationOperationEnvelope)   ← the catalog below
  RecoveryBarrier                          ← pick a checkpoint after this unit
  ModuleFact                               ← reserved module hook
  Default                                  ← test / future compatibility
```

A `FederationOperationEnvelope` is `{ federation_id, version, operation }`. Its hash is the `operation_id`. Same bytes, same ID. Exact retries are duplicates. A different payload is a different operation.

Wallet fan-out, DKG, FROST signing rounds, peer status, and catch-up are **not** consensus items. They ride the private REST/Iroh plane. Consensus only orders the facts those planes later depend on.

### The operation catalog

Grouped by job, not by enum order.

**Wallet value operations** — one exact Cashu request becomes one envelope. Typically submitted by the member that received the public HTTP call. Other members who receive the same request submit the same ID and join it.

| Kind | What it commits | Who submits | Conflict lock |
|---|---|---|---|
| `MintQuote` | quote id, request, response | first healthy member | quote id |
| `Mint` | paid quote(s) + exact blinded outputs | every member that was asked to mint; one winner | each quote id |
| `MeltQuote` | quote id, request, response | first healthy member | quote id |
| `Melt` | quote + input proofs + optional change | every member asked to melt; one winner | quote id |
| `Swap` | input proofs + witnesses + outputs | every member asked to swap; one winner | spent proofs at apply |

**Treasury observations** — one envelope *per observer*. Quorum is counted after they are accepted, not by stuffing many observers into one message.

| Kind | What it commits | How many |
|---|---|---|
| `MintQuotePayment` | observer, quote, method, state, amounts, payment id | one per observing member |
| `MeltQuotePayment` | observer, quote, method, state, amounts, proof | one per observing member |
| `ChainObservation::Deposit` | observer, outpoint, amount, script, block, depth, allocation | one per observing member |
| `ChainObservation::Reorg` | observer, outpoint, epoch | one per observing member that saw the disconnect |
| `TransactionBroadcastObservation` | observer, exact txid / broadcast result | one per observing member |

Production `member_quorum` must be at least `consensus_threshold`. Development `development_single_observation` allows one observer. Fakewallet uses that.

**On-chain custody (federated BDK)** — usually one proposer, then every member applies the same bytes.

| Kind | What it commits | Who submits |
|---|---|---|
| `AddressAllocation` | application epoch, keychain, index, quote or change owner | the member creating the quote / change |
| `TransactionProposal` | exact unsigned tx, inputs, dest, fee, sighashes, optional change | a watch-only proposer |
| `TransactionSigned` | the exact signed bytes + per-input audit | a member that assembled the FROST signature |
| `TransactionBroadcastIntent` | authorization to send *those* bytes | any ready member after persist |

FROST round-one / round-two packets are private REST after `TransactionProposal` is accepted. They are not journal operations.

**Bark Lightning (generic `Application` / `ExternalApplication`)** — same AlephBFT log, different payload family.

| Bark kind | What it commits |
|---|---|
| `ReceiveIntent` | receive secret, locator, amounts, policy, before invoice |
| `ReceivePrepared` | exact invoice / payment-hash identity |
| `ActionPlan` | one send, claim, revoke, refresh, or exit plan |
| `ActionTransition` | one semantic step of that plan |
| `MemberObservation` | one member's Ark / HTLC / chain evidence |
| `EvidenceCertificate` | quorum certificate over independently checked evidence |

Action kinds: `LightningSend`, `IncomingClaim`, `FailedSendRevocation`, `Refresh`, `EmergencyExit`.

**Housekeeping**

| Kind | What it commits |
|---|---|
| `KeysetRotation` | complete new v3 aggregate keyset + every member's public BLS shares |
| `RecoveryBarrier` | select a recovery checkpoint after this unit |

### Who has to show up

For `n` members, BFT uses `C = consensus_threshold = n - floor((n-1)/3)`. Example: 5 members → `C = 4`, at most 1 Byzantine. BLS/FROST signing uses `T = signature_threshold`, `T ≤ C`. Payment observation quorum `Q` is configured; production requires `Q ≥ C`.

- **Ordering** a unique operation: `C` members must finalize the unit that contains it. One member proposes; the rest agree.
- **Observations**: `Q` *distinct* accepted observer envelopes. Not `Q` signatures on one envelope.
- **BLS shares**: after the value operation is accepted, any `T` ready members can finish the Cashu proof. The other `n-T` may be offline.
- **FROST shares**: after `TransactionProposal` is accepted, `T` members sign the Bitcoin tx. Still not a consensus item.

### Happy-path counts

Counts are *distinct accepted operations*, not HTTP calls and not AlephBFT network packets. Duplicates of the same `operation_id` do not add another fact.

**Swap, any backend: 1**

```
1 × Swap
```

Proposed by each member the wallet fanned out to; the log stores one. Then `T` BLS shares off-log.

**Mint, fakewallet / single observation: 3**

```
1 × MintQuote
1 × MintQuotePayment     (one observer)
1 × Mint
```

**Mint, federated BDK: 3 + D + Q**

```
1 × MintQuote
1 × AddressAllocation
D × ChainObservation::Deposit     (each watcher; D is usually n or Q)
Q × MintQuotePayment              (each observer)
1 × Mint
```

Typical 3-of-5 with `Q = C = 4`: about **1+1+4+4+1 = 11** if four members both see the UTXO and vote paid. A reorg adds `Reorg` observations and a new deposit epoch; it does not unmint after issuance.

**Melt, fakewallet / single observation: 3**

```
1 × MeltQuote
1 × Melt                  (spend proofs, maybe BLS change)
1 × MeltQuotePayment
```

**Melt, federated BDK: 5 + B + Q**

```
1 × MeltQuote
1 × Melt                  (proofs + payment intent; no coins move)
1 × AddressAllocation     (internal change, if any)
1 × TransactionProposal
1 × TransactionSigned
1 × TransactionBroadcastIntent
B × TransactionBroadcastObservation and/or confirmation observations
Q × MeltQuotePayment
```

Plus off-log FROST rounds per input. One member cannot skip from `Melt` to broadcast.

**Mint, federated Bark: quote + receive family + claim + issuance**

```
1 × MintQuote
1 × Application ReceiveIntent
1 × Application ReceivePrepared
Q × Application MemberObservation
0–1 × Application EvidenceCertificate
1 × Application ActionPlan (IncomingClaim)
k × Application ActionTransition
Q × MintQuotePayment
1 × Mint
```

`k` is the number of accepted claim steps, not one HTTP round-trip.

**Melt, federated Bark: quote + send family + settlement**

```
1 × MeltQuote
1 × Melt
1 × Application ActionPlan (LightningSend)
k × Application ActionTransition
Q × Application MemberObservation
0–1 × Application EvidenceCertificate
Q × MeltQuotePayment
```

Failure adds `FailedSendRevocation` (another plan + transitions). Refresh / emergency exit are separate action lineages, not part of a normal user melt.

### How AlephBFT groups them

Items sit in a pending mempool, then in an ordered unit. A unit may contain several items. Wallet mint/swap/melt are usually proposed one envelope at a time. Bark can publish an *ordered group* that must finalize together. Duplicate `operation_id`s already in the mempool are joined, not re-queued. Conflicting keys in the mempool are rejected before ordering.

Catch-up replays the same accepted envelopes in journal order. It does not invent new kinds.

### What is deliberately not in this log

- Wallet `POST /mint`, `/melt`, `/swap` bodies
- Member BLS signature shares (stored against an accepted `operation_id`, not ordered as their own item)
- FROST DKG and FROST signing round packages
- BLS DKG commitments / reveals / secret shares
- Peer status, AlephBFT network frames, catch-up certificates (certificates *cover* the log; they are not operations)

One-liner: consensus messages are the named facts; HTTP and FROST are how members learn and how they sign after the fact is accepted.

---

## Roster and federation config

The federation is not "whoever answers at these five IPs today." It is a frozen public document. Every consensus envelope carries `federation_id`. A member that is not on that document cannot authorize operations. A config that does not hash to that ID is a different mint.

### What the ID actually binds

`FederationId` is SHA-256 over a domain-separated transcript of:

- setup authorization (the unanimous setup proposal)
- `member_count`, `signature_threshold`, `consensus_threshold`
- for each member, in order:
  - `member_id`
  - `public_mint_url`
  - `federation_api_url`
  - secp256k1 `identity_public_key`

Change any of those strings or keys and you do not have an updated federation. You have a new `federation_id`. Old journal entries, old DKG transcripts, old FROST roots, and old wallets do not follow you.

There is a second hash, `config_digest`, over the *entire* `FederationConfig`. Wallets check that digest in each member's mint-info advertisement. So even fields that are *not* in the ID still have to match across the roster, or wallets refuse to aggregate.

Full public config today:

| Field | In `federation_id`? | In `config_digest`? | Live mutation path? |
|---|---|---|---|
| setup authorization | yes | yes | no, new setup |
| thresholds | yes | yes | no, new setup |
| member IDs | yes | yes | no, new setup |
| public mint URL | yes | yes | no, new setup |
| private federation URL | yes | yes | no, new setup |
| identity public key | yes | yes | no, new setup |
| wallet protocol version | no | yes | no, new setup |
| payment observation policy | no | yes | no, new setup |
| checkpoint policy | no | yes | no, new setup |
| FROST / on-chain wallet policy | no | yes | no; root is immutable |
| BLS keysets | no | yes | yes: `KeysetRotation` |
| setup transcript | no | yes | written once at activation |

The only consensus mutation of the public signing surface is keyset rotation. There is no "change member 3's DNS" operation. Setup docs say it outright: no dynamic membership, no in-place roster change, no automatic exclusion. Lose a member or change the roster and you abort, agree a new roster, and run setup again.

### Why config is consensus

If the roster lived only in each operator's `config.toml`, members could disagree about who is allowed to submit, who may observe payments, and which URL is canonical. Then:

- wallets would fan out to different sets of people;
- catch-up would not know which peer identity is legitimate;
- a Byzantine member could advertise a look-alike roster.

So the public config is part of the agreed world. Envelopes bind `federation_id`. Startup, DKG, catch-up, and wallet import fail closed on digest / roster / transcript mismatch. They do not "repair" a URL.

### The DNS example

HTTPS federations put domain names in both URLs. Those strings are inside `federation_id`.

`mint-a.example.com` expires, the operator wants `mint-a.net`:

- update local nginx → the other members still dial the old host;
- edit one member's config → their derived ID no longer matches;
- tell wallets the new URL → mint-info digest no longer matches the imported federation.

There is no rotation for locators. The honest path is a new setup: new proposal, new authorization, new ID, new dual DKG, migrate users. Same if someone pasted raw IPs into the roster and then the VPS moved. Same if Iroh is used and an `iroh://<endpoint-id>` changes because the node key was regenerated.

Iroh helps only in the sense that the locator is a stable endpoint ID, not a rented domain. It does not make locators editable. A new endpoint ID is still a new roster row.

TLS certificates themselves are not in the hash. You can renew Let's Encrypt for the *same* name. You cannot change the name.

### Other freeze-or-recreate decisions

Same shape as DNS, some already frozen, some still a product TODO.

Already frozen, so they also imply a new federation:

- **Add, remove, or replace a member.** No membership op. Dead operator → new roster + new DKG. Users must move eCash; BLS and FROST shares do not transfer.
- **Rotate a member identity key.** That key authenticates private REST. It is in the ID. Compromise means new setup, not a rekey op.
- **Change `T` or `C`.** Thresholds are genesis. 3-of-5 to 4-of-7 is a new mint.
- **Change payment-observation quorum or policy family.** `member_quorum` vs `development_single_observation` is in the digest. Softening Q is not a local tune.
- **Change Bitcoin network, confirmation depth, fee policy, FROST application epoch.** `frost_wallet` is digest-bound and the FROST root is immutable. No in-place custody rotation.
- **Change wallet protocol version.** Wallets require an exact version.
- **Swap HTTPS ↔ Iroh, or merge public and private URLs.** Both URLs are hashed; they must stay distinct.

Still open — these should be decided before calling the roster "done":

- **Locator vs identity.** Today they are glued. A future design could hash only member ID + identity key, and treat URLs as a rotatable advertisement. That is the real fix for DNS. It does not exist yet. It needs an authenticated "this key now lives at this URL" update that wallets and peers both accept.
- **What belongs in genesis vs what may be policy.** Observation quorum, checkpoint cadence, fee caps, confirmation depth: some of these might want a unanimous or `C`-of-`n` config-amendment operation instead of a new DKG. None of that is specified.
- **Member repair vs federation replacement.** Can a lost disk restore from catch-up plus sealed backup, or must that seat be burned? Catch-up can restore *state* for an existing identity. It cannot issue a new identity into the old ID.
- **User-visible federation identity.** Wallets import `federation_id` + digest. There is no "same mint, new URL" story. How do users migrate proofs if operators must recreate?
- **Partial transport failure.** If only the private URL dies, members cannot consensus. If only the public URL dies, wallets cannot fan out. Because both are genesis, you cannot fail over to a backup name.
- **Should `config_digest` include keysets?** Rotation changes keysets, therefore the digest. Wallets already re-validate advertisements. That path works, but it means "the federation document" is not quite immutable after genesis — except the ID is. Operators need that distinction burned into their heads.
- **Iroh tickets / relay addresses.** If tickets stay out of the hashed URL, they can be redistributed. If anyone bakes a ticket or relay into the roster string, rotating relays becomes a new federation.

### Presentation point

The roster is the constitution. Consensus does not elect new members and does not update DNS. Keyset rotation changes what eCash key signs. Almost everything else — who the members are, how to reach them, how many must agree, how payments are observed, which Bitcoin network the treasury uses — is setup-time law. If you put a hostname in the constitution, changing the hostname founds a new country.

---

## Crash resistance, backup, and catch-up

A federation stays up if fewer than the BFT fault bound crash. That is liveness. Recovering *one* crashed member is a different problem: that seat still owns unique secrets. Peers can replay history to it. They cannot give it a new identity or new BLS/FROST shares.

There is no BIP39 seed that regenerates a federation member. Standalone CDK can derive mint keys from a mnemonic. A federation member is a named roster seat. Lose the secrets for that seat and you do not restore the node. You replace the federation.

### What must be backed up

Treat these as one generation. A database without the matching secrets, or secrets without the matching database generation, fails startup.

**Offline / offline-equivalent secrets (cannot be caught up from peers):**

- `MemberConfigPrivate`
  - `member_id`
  - secp256k1 `identity_secret_key` (authenticates private REST; must match the roster public key)
  - `keyset_secret_shares`: this member's BLS share for every amount of every keyset, including rotated inactive ones still needed to verify or restore old proofs
  - setup transcript hash that those shares came from
- Public `FederationConfig` (or a byte-identical copy). The backup manifest binds its digest.
- FROST, if the joint treasury is enabled:
  - sealed local root signing share
  - the sealing key / sealed-storage generation that can open it
  - public FROST root package (usually in config / DB, but the *share* is not peer-recoverable)
- Bark receive secrets / mailbox keys, if Bark is enabled. Those are member-local and required to claim incoming HTLCs after restore.
- TLS / Iroh node keys if those locators are how the roster finds you. They are not in `MemberConfigPrivate`, but losing them is a locator problem on top of restore.

**Quiesced database snapshot:**

Stop the mint, snapshot SQLite/Postgres, keep it stopped while you write the manifest. The snapshot holds:

- accepted journal from genesis (v1 retains index 0)
- finalized AlephBFT item bodies from genesis
- materialized Cashu state (quotes, proofs, shares, sagas)
- address-allocation / UTXO / payment-intent projections
- scheduled and trusted checkpoint quorums
- active AlephBFT session backup
- schema contract and durable frontier cursors

**Backup manifest** (`cdk-mintd federation backup-manifest`):

Not a secret file, but required. It binds federation ID, config digest, member ID, trusted checkpoint, exact durable frontier, active AlephBFT backup size, schema contract, and a commitment to the external secret generation. Restore checks this before DKG reconciliation or serving traffic. No bypass flag.

Store snapshot + manifest + secret files together. Protect all of it.

### What the member may lose

| Lost | Recoverable? | How |
|---|---|---|
| Process crash, DB intact | yes | ordinary restart |
| In-memory AlephBFT suffix | yes | replay retained suffix from trusted checkpoint + session backup |
| Journal / DB after a good snapshot | yes | restore snapshot + manifest + secrets, then catch up the tail |
| Entire DB, secrets intact | yes, slow | empty DB + genesis catch-up pages from peers |
| Network partition / lag | yes | peer catch-up, fail closed until caught up |
| Pending unfinalized proposal | maybe | exact `operation_id` retry joins; do not submit a mutated envelope |
| FROST nonce mid-round | yes, conservatively | ambiguous nonces burn; new attempt; never reuse |
| One member offline | yes for the federation | remaining `C` / `T` keep going |
| Identity key or BLS shares | **no** | not reconstructible; new federation |
| FROST sealed share or sealing key | **no** for treasury | cannot sign Bitcoin/Lightning as that seat |
| Another member's secrets | never use them | wrong seat; activation / PoP fail |

A checkpoint is a *commitment* to state, not a snapshot. An empty database cannot start from a checkpoint ID alone. It needs the bodies, or a real DB backup that already contains them.

### Mechanisms

**1. Ordinary restart**

Startup reloads DKG/private config, public config, session markers, pending ops, finalized history, application state, catch-up evidence. If the DB is intact it verifies the trusted checkpoint quorum, roots consensus there, and only walks the retained suffix plus the active AlephBFT backup. Exact-checkpoint restart does not reread the whole journal prefix.

Fail closed if evidence is missing, digests diverge, versions are unsupported, or readiness is stale. No public submit, no FROST, no observations, no broadcast until ready.

**2. Deterministic journal replay**

Accepted envelopes apply in one order, idempotently. Replay of the same `operation_id` reproduces the same materialized state and the same stored BLS shares. That is why a restored member can rebuild quotes, spent proofs, and share rows from history instead of trusting a loose SQL dump.

**3. Order digest + materialized-state digest**

The chain of operations has an order digest. Deterministic DB state has a materialized-state digest. Catch-up and checkpoints bind both. A peer that has the same journal but a different SQL projection is not caught up.

**4. Peer catch-up**

Lagging member asks peers for finalized AlephBFT items plus compact operation metadata. Peers also send *catch-up certificates* for a contiguous range: federation, range, ordered operation IDs, order digest, state digest, signer. The recovering member needs a consensus-threshold certificate quorum, then replays, then checks both digests before becoming ready.

Catch-up and live finality are serialized. A member must not apply a live unit and a catch-up page at the same time.

**5. Checkpoints**

After a unit is fully applied, the checkpoint manager may cut a scheduled checkpoint: signed commitment to that boundary, lineage, policy, and state digest. Promotion is transactional. Later restarts treat that as the trust root and only verify the suffix.

A recovery checkpoint can be cut to help a lagging peer, but a newer scheduled checkpoint wins lineage. Checkpoints authenticate; they do not ship the database.

v1 retains genesis as the bootstrap floor: journal and finalized bodies from 0, historical checkpoint quorums kept. No silent pruning API. Active AlephBFT session backup is protected.

**6. Checkpoint-bound DB restore (disaster)**

Restore the quiesced snapshot, public config, private config, FROST sealed material, set `restore_manifest_path`. Startup validates manifest vs DB vs secrets first. Wrong ID, digest, checkpoint, frontier, schema, backup size, member, or secret generation refuses to start.

Then run `federation audit-checkpoint`. Only return the member to service on a matched audit.

**7. Empty-database genesis replay**

New empty DB + old secrets. Download bounded checkpoint pages of journal / finalized bodies from peers, verify with the same suffix verifier used at startup, apply from 0. Checkpoints tell you the page is authentic. They do not invent BLS shares.

**8. AlephBFT session backup**

Opaque current-session backup lets the consensus engine resume. It cannot advance journal authority, application state, or checkpoint trust by itself. Missing/invalid backup may block engine recovery; it must not rewrite Cashu state.

**9. Readiness / fail-closed gates**

Public mint/swap/melt, FROST commitments/shares, payment observations, and broadcast all require: not lagging, not catching up, not halted, checkpoint/audit OK. This is the crash-safety valve. A half-restored node does not sign.

**10. Idempotent retries and replay fences**

Same envelope → same `operation_id` → join the in-flight or accepted result. Private REST has durable sequences for effectful DKG and kind-isolated volatile windows for status/catch-up/Aleph. Exact FROST commitment/share replay is allowed; a different payload is equivocation.

**11. Persist-before-send / persist-before-broadcast**

DKG and FROST phase state is sealed to disk before dependent outbound messages. Signed Bitcoin bytes are persisted (`TransactionSigned`) before `BroadcastIntent`. A crash cannot create a signature the log does not know about, or broadcast bytes that were never accepted.

**12. FROST nonce lifecycle**

`vacant → reserved → published → consumed`. Timeout, restart, or ambiguity burns the nonce. Prevents the classic crash-and-reuse nonce leak.

**13. Materialized-state audit latch**

If an operator audit (or equivalent) sees state ≠ digest, the member latches failed. Restart, peer agreement, and checkpoint promotion do not clear it. There is no "force clear." Preserve the bad DB, restore a known-good generation or genesis-replay with the *same* secrets, audit again.

**14. Wallet-side (not member restore, but related)**

Wallet sagas already tolerate ambiguous federation responses. Restore of blinded messages returns shares only for outputs already signed by accepted operations. That recovers *users* after wallet crash. It does not recover a mint member.

### Restore cookbook

**Process died, disk fine:** start mintd. It should come ready after suffix verify. If it stays fail-closed, it needs catch-up, not a new identity.

**Disk lost, secrets and a quiesced backup exist:** restore DB + configs + sealed FROST + manifest. Audit. Then let it catch up from the snapshot frontier to tip.

**Disk lost, only secrets exist:** empty DB, genesis page catch-up, same secrets. Long. Possible in v1 because history is retained from 0.

**Secrets lost:** stop. Do not copy another member's private config. That seat is dead. Remaining members can keep serving until you run a new setup with a new roster.

### One-liner

Peers can give you the *book*. Only your backup can give you the *pen*. The book is the journal, checkpoints, certificates, and catch-up. The pen is identity, BLS shares, and the sealed FROST root.
