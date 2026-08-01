# Nocean multi-operator federation setup drill

## Result

The 2026-08-01 invitation-based setup rehearsal passed on source revision
`5d4381051d32a5d8e50bef76cae69b213fa72949`.

This was a **single-host isolated-session rehearsal**, not a multi-machine run.
The configured Nocean access exposed one unique machine identity. The strict
deployment default requires five and aborts before build or mutation when that
requirement is not met. This run used the explicit one-machine rehearsal
override and therefore does not close the multi-host gate.

No invitation, credential, private key, private config, machine identity,
server address, operator path, wallet seed, or generated deployment artifact is
included in this report.

## Topology and operator UX

- Five members with a BLS threshold of three.
- Five protected operator directories and five independent named tmux
  sessions; a separate tmux session held the wallet smoke.
- Each operator generated its setup identity, runtime seed, SQLite state,
  activated private config, and backup material inside its own environment.
- The deployment driver transferred only protected invitation files. All other
  cross-operator data used authenticated setup, DKG, and runtime protocol
  messages.
- Five public mint HTTPS virtual hosts and five distinct private federation
  HTTPS virtual hosts terminated at the Nocean reverse proxy and forwarded to
  operator-local loopback listeners.
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
| Proposal/setup authorization | `39049ad2c7630391269a05b392087226814650e1a3b8c2184df330820be99de7` |
| Certified manifest | `f620dbaaa03f53cc39955ae78d8c4b57156670972e04dc4603349543f62655fd` |
| DKG transcript | `238b512f71607c10a1ebb1d01a1ab8ecb720b88b71f33b551d4427bdca7360cc` |
| Final config digest | `849d66885b59a7d3dc93b24e707233bac7fe1b29b0c00750e6deebbef377e44a` |
| Byte-identical public config | `cc83cf19d80d1be3271dfe69f4c65e0bb25499a4c0ee8d41d31bbfce82ef02f1` |

The DKG phase took 19 seconds. The full driver run, including cached release
build verification, transfer, failure drill, setup, DKG,
activation, runtime checks, wallet checks, restarts, backup, and report
generation, took 239 seconds. Sampled peak DKG RSS was 215,692 KiB across all
five services and 43,824 KiB for one service.

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

The live process exposed and fixed deployment-only integration gaps:

- the Linux container build now mounts the optional Bark path dependency
  read-only because Cargo resolves the workspace path even without Bark
  features;
- tmux sessions start with a stable idle pane before applying window policy;
- authenticated setup readiness recognizes `401` while coordinator-loss checks
  require both `200` and `401` to disappear;
- the five distinct federation TLS virtual hosts are validated rather than
  reusing public mint identities;
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

## Remaining limit

The setup UX, identity isolation, concurrency, restart, and backup behavior are
verified on Nocean, but only one physical machine was reachable. A closing run
still needs five supplied SSH operator environments (or at minimum a genuinely
multi-machine topology) with the same public/private TLS routing. The driver
already rejects a false multi-host claim and accepts exactly five SSH aliases,
so no protocol or deployment-code change is required for that rerun.
