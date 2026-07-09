# Federated Cashu CLI Command-Routing Audit

Date: 2026-07-10

## Objective

Every `cdk-cli` command must do one of the following when its mint URL belongs
to an imported federation:

1. use the imported federation wallet and its `FederatedMintConnector`;
2. remain purely local and avoid unnecessary federation activation; or
3. fail closed with a clear standalone-only error when federation semantics are
   not defined.

This audit covers mint-facing transport. Direct connections to non-mint
services such as Nostr relays, NpubCash, DNS, or an OIDC provider are outside
member fanout, but any resulting Cashu mint interaction must still use the
federation connector.

## Command Inventory

| Command | Classification | Federation behavior |
|---|---|---|
| `decode-token` | Local-only | Decodes supplied data without activating federation wallets or recovering wallet sagas. |
| `balance` | Wallet repository | Uses startup-activated wallets, including imported federation wallets. |
| `federation` | Registry management | Imports, validates, lists, and removes public federation metadata without activating all wallets. |
| `melt` | Value operation | Uses the selected wallet connector for quote, proof selection, threshold fanout, change-share aggregation, and status recovery. |
| `mint-pending` | Value recovery | Iterates repository wallets; imported federation wallets retain their threshold connector. |
| `receive` | Value operation | Resolves a token URL against imported local and member aliases, then receives or swaps through the federation connector. |
| `send` | Value operation | Uses the selected wallet connector; online sends perform federation swaps and aggregate threshold shares. |
| `transfer` | Cross-mint value operation | Resolves explicit source and target member aliases through `get_or_create_wallet`; either side may independently be federated or standalone. |
| `check-pending` | Value recovery | Checks pending proofs through each activated wallet connector. |
| `check-state` | State query | Resolves local or token proofs through the imported federation and requires threshold-consistent member state. |
| `check-requests` | Payment-request recovery | Uses repository wallets, including activated federation wallets. |
| `mint-info` | Read-only mint query | Constructs the connector directly from stored federation metadata and requires threshold-valid mint-info/keyset advertisements; it skips eager activation. |
| `mint` | Value operation | Creates and polls the quote through the federation connector, then aggregates threshold-valid v3/BLS shares. |
| `mint-batch` | Value operation | Uses the federation connector for batch issuance and threshold-valid share aggregation. |
| `burn` | Value operation | Uses the selected repository wallet and its connector for proof-state handling. |
| `restore` | Value recovery | Uses the federation connector and threshold-consistent restore responses. |
| `update-mint-url` | Standalone-only | Rejects imported federation URLs. Member URLs and federation identity are fixed by the validated public config and cannot be mutated as one standalone mint URL. |
| `list-mint-proofs` | Local-only | Reads the local wallet database; startup activation preserves repository wallet construction. |
| `decode-request` | Local-only | Decodes supplied data without activating federation wallets or recovering wallet sagas. |
| `pay-request` | Value operation | Selects a repository wallet and uses its connector for the resulting Cashu payment. |
| `resolve` | Wallet-assisted query | Uses the selected wallet context; BIP353/DNS resolution is not federation member transport. |
| `create-request` | Local/wallet operation | Builds request metadata from repository wallets; Nostr publication is not federation member transport. |
| `mint-blind-auth` | Standalone-only | Rejects imported federation URLs because federation-wide blind-auth issuance and aggregation are not defined. |
| `cat-login` | Standalone-only | Rejects imported federation URLs because one member's OIDC login is not a federation authentication protocol. |
| `cat-device-login` | Standalone-only | Rejects imported federation URLs for the same authentication reason. |
| `npub-cash` | External service plus mint wallet | NpubCash traffic remains direct to that service; all mint-wallet lookup paths resolve imported member aliases through `get_or_create_wallet` and use the federation connector. |
| `generate-public-key` | Wallet repository | Uses the selected repository wallet; imported federation aliases resolve during startup activation. |
| `get-public-keys` | Wallet repository | Uses `get_or_create_wallet`, which injects the imported federation connector. |

## Closed Routing Gaps

- NpubCash wallet helpers no longer create a standalone HTTP wallet for an
  imported federation member URL.
- Explicit `transfer --source-mint` and `--target-mint` URLs now recognize
  imported federation member aliases and construct the correct connector.
- CAT login, CAT device login, blind-auth minting, and mint-URL mutation reject
  imported federations before direct member HTTP or local wallet mutation.
- Local decode commands and standalone-only commands skip eager federation
  activation; local decode and standalone-only commands also skip unrelated
  saga recovery.
- Wallet repository reconstruction uses persisted mint metadata and does not
  issue a direct `/v1/info` request before imported federation connectors are
  installed.

## Test Gates

- Federation and non-federation CLI builds compile without cfg-only warnings.
- Registry tests prove standalone-only guards reject imported federation URLs
  while standalone URLs remain accepted.
- NpubCash helper tests prove both normal and no-probe wallet lookup construct a
  `FederatedMintConnector` for imported member aliases.
- Startup classification tests prove read-only/local and standalone-only
  commands do not eagerly activate federation wallets; commands that need
  wallet networking continue to activate them.
- Wallet-library connection-counter tests prove repository reconstruction does
  not contact a mint for either persisted `MintInfo` or missing-metadata rows.
- The full federation-focused CLI suite passes, including mint, send/swap,
  melt, restore, check-state, invalid-share, keyset validation, member-routing,
  and no-quorum regressions.
- The existing live AlephBFT CLI smoke matrix remains the process-level gate for
  ordinary token compatibility and mint/swap/melt behavior.

## Result

No mint, swap, melt, restore, or proof-state CLI path intentionally bypasses
the imported federation connector. Commands without defined federation-wide
authentication or URL-mutation semantics fail closed instead of silently
contacting one federation member as a standalone mint.
