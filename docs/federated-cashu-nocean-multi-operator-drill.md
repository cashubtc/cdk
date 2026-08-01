# Nocean multi-operator federation setup drill

## Result

The 2026-08-01 invitation-based Iroh setup deployment passed on source revision
`5df42fdb4a7d1d91efa44689b5fa5ce44b769c3c`.

This was a **single-host, five-operator deployment**, not a multi-machine run.
It is the accepted cost-constrained topology for the current federation-setup
milestone: one Nocean server, five independent operator environments, and
five existing public mint DNS subdomains. Setup and every private federation
route used five distinct persistent Iroh N0 endpoints; no private federation
DNS name or certificate was used. The strict deployment mode still requires
five unique machines for a future parent-plan production run. This result
closes the Iroh multi-operator setup/DKG deployment gate, but it does not close
that separate multi-host gate.

No invitation, credential, private key, private config, machine identity,
server address, operator path, wallet seed, or generated deployment artifact is
included in this report.

### All-Iroh public-wallet follow-up

A fresh follow-up deployment passed on clean source revision
`e4784e84f1d7ef19688adc243bc869aade817acb`. It retained the accepted
single-host, five-operator and threshold-three topology, but removed the public
HTTPS dependency: each member advertised its persistent Iroh endpoint for both
public wallet and private federation traffic. The setup coordinator, all five
concurrent DKG services, and all five runtimes had HTTP disabled. The driver
verified that every configured TCP mint port remained closed in each phase.

The Iroh-enabled TUI discovered the public federation through a member Iroh
URL, required all five advertised public URLs to use Iroh, loaded threshold
mint information, and repeated the read with one member offline. An independent
TUI process on the development machine then connected to the deployed Iroh
entrypoint and validated the five-member roster and empty balance. No DNS name
or HTTPS reverse proxy participated in these federation or wallet checks.

Concurrent dealer-free BLS/FROST DKG took 23 seconds. The complete clean Linux
release build, transfer, recovery drill, setup, DKG, activation, wallet checks,
outage/catch-up, coordinated restart, backup/restore, and report generation
took 1,219 seconds. Sampled peak DKG RSS was 296,300 KiB across the five
services and 60,620 KiB for one service. As with the earlier run, this proves
operator/process isolation on one affordable server, not multi-machine fault
domains or a funded Signet paid-wallet flow.

## Topology and operator UX

- Five members with a BLS threshold of three.
- Five protected operator directories and five independent named tmux
  sessions; a separate tmux session held the wallet smoke.
- Each operator generated its setup identity, runtime seed, SQLite state,
  activated private config, and backup material inside its own environment.
- The deployment driver transferred only protected invitation files. All other
  cross-operator data used authenticated setup, DKG, and runtime protocol
  messages.
- Five public mint HTTPS virtual hosts terminated at the existing Nocean
  reverse proxy. Setup, DKG, FROST, AlephBFT, checkpoint/catch-up, status, and
  other private federation calls used Iroh endpoint URLs directly.
- The operator panes executed the normal public commands: `setup create`,
  `join`, `propose`, `review`, `approve`, `status`, `serve`, and `activate`.
- After certification, the coordinator was stopped before all five DKG services
  were started concurrently. DKG did not use member-by-member orchestration.

The post-drill audit found five operator sessions, five live `cdk-mintd`
service windows, five healthy public HTTPS routes, five passing strict
ready/converged federation status checks, and the separate wallet session.

## Evidence

| Evidence | Value |
|---|---|
| Proposal/setup authorization | `00dd0da71bbc84c5ecf34805c79a496b10afc540cfd39a365e513151370ec29d` |
| Certified manifest | `25ae8923a5b074ec7357766cff3e711bb3097cbd89fbec9193dcb8932653aa0a` |
| DKG transcript | `b81d1f88d7bfd29b8d7039019c62c05b49069ee2c8b88829e6883ecb150ee984` |
| Final config digest | `ae4137b62b1498a9fff5b68a912110a038f1eb82b7c0e2b3d3c2b7c062f3e6d9` |
| Byte-identical public config | `baba0460a383c081ea0e9ca35a931eb88ab30768321cdec6335989a6f9701e8b` |

The concurrent DKG/FROST phase took 21 seconds. The full driver run, including
a clean Linux release build verification, transfer, failure drill, setup, DKG,
activation, runtime checks, wallet checks, restarts, backup, and report
generation, took 938 seconds. Sampled peak DKG RSS was 295,004 KiB across all
five services and 59,544 KiB for one service.

## Failure and recovery drills

- A four-of-five pre-certification enrollment could not produce a proposal.
  Operators then used a new setup ID; no private state was copied forward.
- Every operator reviewed the same proposal hash and approved it once. Every
  member persisted the certified manifest before coordinator removal.
- DKG completed after coordinator loss because all five members independently
  held the signed manifest.
- Every member explicitly activated the final configuration, and all activated
  public files were byte-identical.
- One runtime was stopped. The configured federation wallet continued to
  perform threshold mint-info and balance reads, then the member restarted and
  passed strict catch-up/convergence checks.
- All five runtimes were stopped and restarted together, then passed public and
  private health checks again.
- Member three created an AES-256-CBC/PBKDF2 encrypted backup and restored it
  locally. Setup config, activated private config, runtime config, and the
  shared setup/runtime SQLite database compared byte-for-byte. Neither the
  encrypted archive nor its local passphrase left that operator environment.

The wallet smoke intentionally did not claim a paid operation. It imported the
federation public config, completed threshold mint-info, verified a new empty
balance, repeated the reads with one member offline, and contained no quote,
payment, or token. Paid Signet/Bark validation belongs to the parent plan's
separate payment-backend gate.

## Issues corrected during the drill

The Iroh rerun retained the earlier deployment hardening and exposed one
transport integration gap:

- a current Iroh endpoint cannot connect to its own endpoint ID, while DKG
  deliberately sends identical signed readiness and artifact requests to every
  member, including self. The first run certified all five operators but stayed
  at the DKG readiness barrier with zero participant states. The generic Iroh
  client now uses one lazy unadvertised companion endpoint only for that local
  target-ID case. The same router, signed request validation, replay fence,
  admission limits, and shared connection pool remain in use; no DKG/FROST
  state machine branch was added. A focused transport regression and a signed
  federation self/remote/replay test cover the behavior.

The deployment tooling also now:

- initializes each protected Iroh endpoint before rendering operator metadata,
  uses N0 discovery without ticket exchange, and keeps all private/setup URLs
  off DNS;
- runs one-shot setup and operator commands through ephemeral configured
  dialing endpoints, while only `setup serve` and the mint runtime own the
  protected persistent identity; and
- checks setup readiness with the authenticated CLI over Iroh rather than
  treating an HTTP status code as reachability.

Earlier hardening retained by this run includes:

- the Linux container build now mounts the optional Bark path dependency
  read-only because Cargo resolves the workspace path even without Bark
  features;
- tmux sessions start with a stable idle pane before applying window policy;
- authenticated setup readiness and coordinator loss are checked through the
  normal setup protocol;
- the active setup ceremony and runtime use the same operator-local SQLite
  database, preserving activation evidence without a copy or migration;
- the runtime payment configuration matches the certified FROST/BDK
  application and does not pretend fakewallet is compatible;
- an empty wallet's empty balance output is accepted without treating arbitrary
  nonzero output as empty; and
- artifact transfers use bounded SCP retries and SSH keepalives, while protocol
  actions retain their explicit replay semantics.

Every failed attempt stopped before the next unsafe boundary. Where identity
state existed, it was moved into that operator's local protected `aborted`
archive rather than deleted or shared.

## Future multi-host extension

The setup UX, identity isolation, concurrency, restart, and backup behavior are
verified on the accepted one-server Nocean topology. This is not evidence of
machine-level fault or administrative independence. The parent plan's future
multi-host production gate still needs five supplied SSH operator environments,
or another genuinely multi-machine topology, with public HTTPS and private Iroh
routing. The driver already rejects a false multi-host claim in strict mode and
accepts exactly five SSH aliases, so no protocol or deployment-code change is
required for that extension.
