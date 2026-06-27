# Native LDK Server Backend Plan

## Goal

Add native `ldk-server` Lightning backend support to `cdk-mintd` so a CDK mint can use an already running `lightningdevkit/ldk-server` daemon instead of embedding its own `ldk-node` instance.

## Approach

Implement Plan B from the brainstorm: a new `cdk-ldk-server` crate that implements `cdk_common::payment::MintPayment` by calling the upstream `ldk-server-client` Rust client.

The backend will be selected with:

```toml
[ln]
ln_backend = "ldk-server"
unit = "sat"

[ldk_server]
address = "127.0.0.1:3536"
api_key = "<hex API key>"
cert_path = "/path/to/tls.crt"
```

## Implementation Tasks

- Add workspace dependency and crate entry for `cdk-ldk-server`.
- Add `crates/cdk-ldk-server` with:
  - `Error` type using `thiserror`.
  - `CdkLdkServer` backend struct.
  - `Config`/builder-style constructor accepting endpoint, API key, cert bytes, and fee reserve.
  - `MintPayment` implementation for BOLT11 and BOLT12.
  - Event stream adapter from `ldk-server` `SubscribeEvents` into CDK payment events.
- Wire `cdk-mintd`:
  - Add `ldk-server` feature.
  - Add `LnBackend::LdkServer`.
  - Add `[ldk_server]` config section.
  - Add `CDK_MINTD_LDK_SERVER_*` environment variables.
  - Register the backend in mint setup.
- Add README/config docs for the new backend.

## Core Mapping

- BOLT11 incoming:
  - `create_incoming_payment_request` calls `Bolt11Receive`.
  - CDK lookup ID uses `PaymentIdentifier::PaymentHash`.
- BOLT12 incoming:
  - `create_incoming_payment_request` calls `Bolt12Receive`.
  - CDK lookup ID uses `PaymentIdentifier::OfferId`.
- BOLT11/BOLT12 outgoing:
  - `make_payment` calls `Bolt11Send` or `Bolt12Send`.
  - CDK lookup ID uses `PaymentIdentifier::PaymentId`.
- Status/events:
  - Outgoing status can use `GetPaymentDetails(payment_id)`.
  - Incoming BOLT12 status needs `offer_id` to payment lookup. Use event-driven mapping when events arrive and bounded `ListPayments` scanning as fallback.

## Risks

- `ldk-server` is upstream WIP and documents API churn risk.
- `GetPaymentDetails` looks up by `payment_id`, while incoming BOLT12 quotes start from `offer_id`.
- The upstream client currently carries older dependency versions, so CDK may compile duplicate versions of `prost`, `reqwest`, `hyper`, and `rustls`.
- Live integration tests need a running `ldk-server`; local CI will focus on compile, unit tests, and mintd config parsing.

## Verification

- `cargo fmt --all -- --check`
- `cargo check -p cdk-ldk-server`
- `cargo check -p cdk-mintd --features ldk-server,sqlite`
- Focused unit tests for config parsing and backend mapping helpers.

## Shipping

- Branch: `codex/ldk-server-backend`
- Remote: `origin` (`git@github.com:hedwig-corp/cdk.git`)
- PR target: `hedwig-corp/cdk:main`
