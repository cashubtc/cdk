# CDK Iroh transport

`cdk-iroh` owns CDK's optional Iroh endpoint, discovery, connection, and
HTTP/WebSocket bridge. It is intentionally transport-only: Cashu routes and
federation state machines remain in their existing crates.

Milestone 0 contains the safe split-stream adapter and an end-to-end feasibility
spike. The production client, server, and lifecycle types are added in later
milestones from `docs/iroh-transport-implementation-plan.md`.

The crate pins stable Iroh 1.0.3 and `iroh-tickets` 1.0.0. This requires CDK's
Rust 1.91 MSRV policy change; see `docs/dependency-decision.md` for the rejected
pre-1.0 alternative and upgrade policy.
