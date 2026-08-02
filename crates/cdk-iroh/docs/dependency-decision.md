# Iroh dependency decision

Decision date: 2026-08-01.

Revalidated against the upstream `latest` package metadata on 2026-08-01:
Iroh 1.0.3 and `iroh-tickets` 1.0.0 remain the current stable releases, and
both declare Rust 1.91. The implementation uses the current standalone
`iroh_tickets::endpoint::EndpointTicket`; it has no dependency on the removed
pre-1.0 in-core ticket feature.

## Decision

CDK raises its declared MSRV from Rust 1.85.0 to 1.91.0 and pins `iroh` to
exactly 1.0.3 and `iroh-tickets` to exactly 1.0.0. Iroh is built without its
default metrics, port-mapping, or fast-Apple-datapath features and explicitly
enables the `tls-ring` provider. Later milestones may enable an operational
feature only with measured evidence.

The version survey found:

| Version | Declared Rust version | Ticket situation | Disposition |
|---|---:|---|---|
| Iroh 1.0.3 / `iroh-tickets` 1.0.0 | 1.91 | Current `EndpointId` and `EndpointTicket` APIs | Selected |
| Iroh 0.98.2–0.96.1 | 1.89 | Post-0.93 APIs | Incompatible with CDK MSRV |
| Iroh 0.95.1 | 1.85 | Standard ticket was removed before `iroh-tickets` was published | Rejected: cannot satisfy static-ticket UX without inventing a format |
| Iroh / `iroh-base` 0.93.2 | 1.85 | Standard `NodeTicket` behind `ticket` feature | Rejected: prerelease Salsa20 conflicts with CDK's stable Salsa20 graph |
| Iroh 0.35.0 | 1.81 | Older API, older discovery stack, and different ticket/wire generation | Rejected as an obsolete production pin |

The first attempted 0.93.2 resolution failed before compilation:
`crypto_box` required `salsa20 = 0.11.0-rc.1`, while CDK's existing `scrypt`
graph requires stable `salsa20 = 0.11`. Cargo cannot select both releases for
that semver line. Patching upstream cryptography or downgrading to Iroh 0.35
would create more security and maintenance debt than the explicit MSRV change.
An optional feature does not bypass Cargo's package `rust-version` contract, so
the selected stable crates require the workspace-wide 1.91 declaration.

## Upgrade policy

Every Iroh upgrade must:

1. preserve parsing tests for exported `EndpointTicket` values or provide an
   explicit operator conversion tool;
2. regenerate connection, discovery, and direct/relay evidence;
3. compare the `cashu-cdk-http/1` ALPN bridge byte-for-byte; and
4. update the exact pin deliberately rather than accepting an unreviewed minor
   networking-stack change.

The higher MSRV drops compiler support for applications fixed below Rust 1.91
and therefore requires release-note and downstream binding communication. In
return, CDK uses the maintained Iroh API and ticket format and can consume
security fixes without a pre-1.0 migration. Exact pins make networking updates
intentional but require prompt review of Iroh and transitive RustSec advisories.
The Nix MSRV toolchain, developer documentation, agent guidance, and lockfile
are updated with the policy change.

## Platform assessment

Iroh 1.0.3 has native Tokio networking paths for Linux and macOS and
target-specific Android and Apple handling. CDK version 1 does not enable
browser/WASM Iroh. Linux and macOS are the initial supported server targets.
Android and iOS remain conditional native targets until CDK's bindings compile
and lifecycle tests pass there; Android applications must install the JNI DNS
context required by Iroh before endpoint construction.

Mobile bindings are outside the initial transport PR and require their own
compile and lifecycle verification before claiming support.
