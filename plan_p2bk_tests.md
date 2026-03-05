# Test Plan: NUT-28 P2BK (Pay-to-Blinded-Key)

This document outlines the testing strategy for the Pay-to-Blinded-Key (NUT-28) implementation, focusing on tests integrated directly into the `cdk` wallet core and tests exercising the functionality from the `cdk-cli`.

## Phase 1: Core Wallet Integration Tests (Completed)

We have verified the functionality of the core wallet APIs to automatically utilize the `use_p2bk` flag in swap sequences.

* **Target File**: `crates/cdk-integration-tests/tests/integration_tests_pure.rs`
* **Test Scenario: `test_p2bk_send_and_receive`**
  1. Initialize an in-memory test mint with a nominal fee (e.g. `fee_ppk=1000`).
  2. Create an Alice (`sender`) wallet and a Bob (`receiver`) wallet connected to this mint.
  3. Fund the `sender` wallet with an initial balance (e.g. 64 sats).
  4. Generate a P2PK receiver `SecretKey` and lock conditions (`SpendingConditions`).
  5. The `sender` prepares a send using `prepare_send` with `use_p2bk: true` and `include_fee: true`.
  6. Verify the proofs inside the generated Token V4 contain the ephemeral public key `p2pk_e`.
  7. The `receiver` calls `receive` using the token and the P2PK signing key.
  8. Assert the balance matches the original amount indicating the `use_p2bk` blind-signature recovery mechanism works automatically during the receive flow.

## Phase 2: CDK-CLI End-to-End Tests (Planned)

Since `cdk-cli` currently lacks an automated integration test harness, we plan to implement a Bash or Python-based end-to-end framework, or add an `assert_cmd`-based test suite to the `cdk-cli` crate. The core goal is to verify that CLI arguments (`--use-p2bk`) accurately translate to the `SendOptions` configurations and result in successful, silent payments.

### Planned Test Suite

1. **CLI Flag Propagation**
   * **Action:** Run `cdk-cli send --amount 10 --pubkey <RECEIVER_PUBKEY> --use-p2bk`.
   * **Verification:** Intercept the `SendOptions` before the wallet prepares the token (or inspect the output token natively) and assert that `use_p2bk` is `true` and the output proofs contain `p2pk_e`.

2. **Full E2E Send & Receive**
   * **Setup:** Start a local `cdk-mintd` instance. Initialize two CLI wallet environments (Alice and Bob).
   * **Action 1:** Fund Alice's wallet with sats.
   * **Action 2:** Alice sends 20 sats locked to Bob's public key with `--use-p2bk`.
   * **Action 3:** Bob receives the token string using his private signing key.
   * **Verification:** Check Bob's balance increased by exactly 20 sats and Alice's balance decreased by 20 sats + fee.

3. **Fallback and Errors**
   * **Action:** Attempt to use `--use-p2bk` without providing a `--pubkey` (or providing an HTLC hash instead).
   * **Verification:** The CLI should error out gracefully before reaching the wallet backend, as P2BK mathematically requires P2PK locks.

### Tooling Recommendations
* Use `assert_cmd` in a new `tests/cli.rs` within `crates/cdk-cli` to spawn CLI processes and test stdout/stderr.
* Set up a lightweight dummy mint via `cdk-mintd` for tests, or mock the database and `wallet_repository`.