# Codex goal prompt: implement federated Cashu in CDK

You are Codex working in the Cashu Development Kit repository. Your goal is to implement federated Cashu support according to `docs/federated-cashu-implementation-plan.md`, step by step, until the whole system is working end to end.

Continue autonomously through the implementation plan. Do not stop after analysis or a partial scaffold. Work milestone by milestone, test each checkpoint, commit each coherent unit of progress, update the progress page, report status, and continue. Stop only when the complete federated Cashu system is implemented, tested, documented, and committed, or when you are genuinely blocked by missing information, unavailable dependencies, or a required design decision that cannot be safely inferred.

## Project vision

Federated Cashu should evolve CDK from single-operator mints into threshold-secured eCash federations where independent mint members run on different servers, agree on every safety-critical state transition through consensus, and jointly mint, swap, and melt tokens with BLS threshold blind signatures. The implementation must improve user security and fault tolerance while preserving the public Cashu wallet-to-mint protocol as much as possible: no single member should be able to inflate or corrupt mint state, and the federation should keep operating when some members are offline as long as the configured thresholds are met. Wallets should fan out requests, verify member shares, aggregate valid threshold shares into ordinary Cashu proofs, and reject inconsistent federation metadata or invalid shares. The project should start with trusted-dealer keys, fake-wallet payments, private REST, and testable local consensus, then grow toward durable storage, catch-up, authenticated member networking, DKG, key rotation, and complete mint/swap/melt federation behavior.

## Recommended `/goal` invocation

For Goal mode, do not paste this entire file into the composer. Use a compact `/goal` command that points Codex at this file and the implementation plan:

```text
/goal Implement federated Cashu in CDK by following docs/federated-cashu-codex-goal-prompt.md as the complete goal specification and docs/federated-cashu-implementation-plan.md as the implementation checklist. Work milestone by milestone until minting, swapping, melting, wallet aggregation, private REST federation networking, consensus, catch-up, tests, docs, and docs/federated-cashu-progress.html are complete. Commit each coherent checkpoint with tests and status updates. Stop only when complete or genuinely blocked.
```

## Non-negotiable working rules

- Follow `docs/federated-cashu-implementation-plan.md` as the source of truth.
- Treat Fedimint as the main reference implementation for federation design questions, especially consensus, DKG, peer authentication, catch-up, state-machine boundaries, and threshold signing safety.
- Prefer CDK/Cashu simplicity over copying Fedimint complexity when a simpler design preserves the same safety property, is easier to test, and fits the existing CDK architecture.
- Preserve standalone CDK mint and wallet behavior unless federation mode is explicitly enabled.
- Enforce the federation keyset constraint: BLS blind signatures are only supported for Cashu keyset v3 (`KeySetVersion::Version02`, keyset ID prefix `02`), so federation mode must only create, advertise, accept, and sign v3/BLS aggregate keysets. Reject v1/v2/non-BLS federation keysets at config load, mint/swap/restore output validation, and wallet federation metadata validation. Standalone non-federated keyset behavior must remain unaffected.
- Keep the interface to existing CDK code as minimal and thin as practical. Organize federation code into logically separated modules with clear ownership and concerns, using narrow feature-gated hooks or adapters instead of spreading broad federation conditionals through existing mint, wallet, HTTP, storage, or signatory code.
- Do not leak or commit secrets, generated private member configs, private key shares, machine-specific paths, IP addresses, device names, usernames, or other personally identifiable local environment details.
- Keep commits small, conventional, and reconstructable.
- Commit after each coherent milestone, independently testable checkpoint, migration, or documentation update.
- Run the relevant tests before each commit whenever practical, and include the test result in the status report and progress page.
- If tests cannot be run, document exactly why and what remains unverified.
- Never use destructive git commands unless the user explicitly asks.
- Never revert user changes. If unrelated files are dirty, leave them alone.

## Status reporting loop

After each checkpoint or meaningful block of work:

- Summarize what was implemented.
- Summarize what tests were run and their results.
- Summarize what changed in the progress HTML page.
- Commit the completed work with a conventional commit message.
- Report the commit hash.
- State the next checkpoint you are starting.

If work takes a long time, provide short progress updates while continuing.

## Ongoing progress page

Maintain an HTML progress page at `docs/federated-cashu-progress.html`.

The page should be modern, readable, and useful as a living engineering dashboard. It should not be a marketing page. It should document the actual implementation status.

Update it after every checkpoint and before every commit.

The page should include:

- Current overall status and latest commit.
- Milestone checklist with completed, in-progress, blocked, and pending states.
- What has been unlocked so far.
- Problems solved.
- Open problems and risks.
- Test matrix and latest test results.
- Architecture decisions made.
- Next planned work.
- Links to the implementation plan and relevant local docs using repo-relative links.

Design guidance for the page:

- Use a clean, modern, information-dense layout.
- Prefer simple HTML and CSS unless the repo already has a docs site framework that should be used.
- Do not require a build step unless there is already a docs build system.
- Do not include secrets, local machine details, private endpoints, or generated private configs.
- Keep it easy to review in a browser from the repository checkout.

## Implementation sequence

Implement the system in the order defined by `docs/federated-cashu-implementation-plan.md`, with this practical execution order:

1. Create or update the progress HTML dashboard.
2. Add feature gates and RFC stubs without changing standalone behavior.
3. Implement threshold BLS primitives and tests.
4. Implement federation configuration, static member roster, identity public keys, partner allowlists, v3/BLS-only aggregate keyset validation, and trusted-dealer test setup.
5. Implement canonical operation encoding and operation hashes for mint, swap, melt, restore, and payment observations.
6. Implement in-memory transport and private REST federation transport behind a common trait.
7. Implement signed member-to-member envelopes and reject all traffic from unknown or mismatched federation partners.
8. Implement a consensus abstraction and an in-memory consensus engine for deterministic tests.
9. Add the federation database journal, session outcomes, state digests, replay, and catch-up primitives.
10. Refactor CDK mint business logic into deterministic state-machine apply steps.
11. Implement federated mint quote creation, fake-wallet paid observations, issuance consensus, signature shares, response caching, and wallet aggregation.
12. Implement sliding-window and conflicting-output mint attack regressions.
13. Implement federated swap, pending/spent transitions, threshold output signing, wallet aggregation, and attack regressions.
14. Implement federated melt with fake-wallet reserve, pending, success, failure, finalization, rollback, change signing, idempotency, and recovery tests.
15. Implement restore and check-state behavior against federated state.
16. Implement networked multi-process fake-wallet federation tooling in `cdk-mintd` or the appropriate test harness.
17. Implement member restart, reconnect, catch-up, lagging-member refusal to sign, and state-digest mismatch handling.
18. Add storage coverage for memory and SQLite first, then Postgres if practical.
19. Add real DKG or a clearly separated DKG scaffold after the trusted-dealer path is fully working.
20. Add key rotation and federation upgrade scaffolding only after the MVP paths are working.
21. Complete observability, operator commands, documentation, and security review checklist.

## Core correctness requirements

The implementation is not complete until these properties are true and tested:

- Federation-aware wallets can mint, swap, and melt through a federation.
- Wallets aggregate BLS blind signature shares into ordinary Cashu proofs.
- Members never return usable signing shares before the relevant operation is accepted by consensus.
- Members serialize all safety-critical state transitions through consensus.
- Members only advertise, accept, and sign Cashu keyset v3 (`KeySetVersion::Version02`) aggregate BLS keysets in federation mode.
- Members reject conflicting output sets for the same mint quote.
- Members reject swap double spends and overlapping-window quorum-cover attacks.
- Members handle fake-wallet mint and melt success, failure, pending, delay, duplicate observation, and retry paths.
- Members can go offline, restart, catch up, converge to the same state digest, and only resume signing after catching up.
- Private federation REST messages are accepted only from configured federation partner identity public keys.
- Unknown members, wrong federation IDs, invalid signatures, stale sequence numbers, and body-hash mismatches are rejected before state mutation.
- Standalone mint and wallet tests continue to pass when federation mode is disabled.

## Expected final result

When the goal is complete, the repository should contain:

- Working federated Cashu mint implementation behind feature/config gates.
- Working federation-aware wallet connector.
- Threshold BLS share generation, verification, aggregation, and tests.
- Federation config/trusted-dealer tooling with static member rosters and identity public keys.
- v3/BLS-only federation keyset validation in config loading, mint/swap/restore output validation, and wallet federation metadata checks.
- Private REST mint-to-mint networking with signed envelopes.
- Consensus-backed mint, swap, melt, restore, and check-state flows.
- Durable journal, replay, catch-up, and state digest checks.
- Fake-wallet federation integration tests covering happy and unhappy payment paths.
- Attack regression tests for mix-and-match/sliding-window threshold signing abuse.
- Updated docs, including `docs/federated-cashu-implementation-plan.md` where needed.
- A polished, current `docs/federated-cashu-progress.html` dashboard.
- All work committed in small, conventional commits.

Begin by reading `docs/federated-cashu-implementation-plan.md`, inspecting the relevant CDK code, creating or updating `docs/federated-cashu-progress.html`, and starting Milestone 0. Continue step by step until completion.
