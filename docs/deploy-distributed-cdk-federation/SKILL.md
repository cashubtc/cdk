---
name: deploy-distributed-cdk-federation
description: Deploy and operate distributed CDK mintd federations across multiple SSH-accessible Linux servers using Iroh-only transport, invitation setup, concurrent DKG, federated BDK and Bark backends, UFW, systemd, rolling restarts, host-key pinning, and mintd-only validation. Use when provisioning, deploying, migrating, auditing, repairing, or documenting a multi-machine CDK federation, especially when adapting the repository's Nocean federation scripts to independent hosts.
---

# Deploy a Distributed CDK Federation

Deploy one independent CDK federation member per server. Keep private operator
state on its originating host, expose the mint through Iroh, and leave only
`cdk-mintd` installed as a server runtime.

Read [references/runbook.md](references/runbook.md) completely before changing
hosts or starting a federation ceremony.

## Non-negotiable rules

- Pin every SSH host key before the first authenticated login. Prefer a
  provider-published fingerprint; otherwise record and independently verify the
  scanned key before continuing.
- Use one dedicated SSH identity. Never print, upload, or commit its private
  key.
- Build Linux release binaries on a capable build machine. Do not compile CDK
  on small federation servers.
- Run one member per distinct machine. Verify distinct machine identities
  rather than trusting host aliases.
- Generate the Iroh identity, runtime seeds, Bark secrets, DKG shares, and
  private federation config on the member that owns them.
- Transfer only protected setup invitations and public verification evidence.
  Never centralize activated private configs or runtime seeds.
- Default to Iroh-only transport: set `http_enabled = false`, keep HTTP and
  Prometheus addresses on loopback, and do not open ports 80, 443, mint,
  federation, database, or metrics ports.
- Default to mintd-only servers. Do not install `cdk-cli`,
  `fed-cdk-tui`, wallet-smoke scripts, or client wallet state unless the user
  explicitly requests server-side client tooling.
- Keep UFW default-deny inbound and default-allow outbound. Permit SSH only.
  Preserve SSH rate limiting by multiplexing deployment connections; use a
  short control path such as `/tmp/cdf/%C`.
- Keep at least the required quorum online during upgrades and service
  migrations. For five members, expect a 3-of-5 signature threshold and a
  4-of-5 consensus threshold unless the user explicitly chooses otherwise.
- Stop before spending funds, replacing an active federation, deleting
  operator state, changing a private key, or widening public firewall access
  unless the user explicitly authorizes that exact action.

## Repository routing

- Study `scripts/deploy_nocean_federation.sh` and
  `scripts/nocean_federation_control.sh` for the proven single-host lifecycle.
  Do not use them as evidence of multi-machine independence.
- Use `scripts/deploy_nocean_multi_operator_federation.sh` as the primary
  multi-host ceremony reference. Check its current behavior before execution:
  its full drill may build or upload client tools that a mintd-only deployment
  must omit.
- Use `scripts/nocean_federation_operator.sh` for member-local setup,
  activation, status, HTTP-listener, and backup checks.
- Install `scripts/cdk-federation-mintd@.service` or an equivalent reviewed
  unit for reboot-safe mintd-only operation.
- Treat `target/` reports as local evidence. Do not commit host addresses,
  account data, SSH material, private configs, seeds, or provider identifiers.

## Workflow

1. Inspect the working tree, relevant scripts, current branch, build inputs,
   sibling Bark checkout, and any existing live federation. Preserve unrelated
   changes.
2. Define the topology, member count, signature threshold, consensus
   requirement, network, Iroh transport, Bark server identity, Esplora source,
   server user, and remote root.
3. Verify every host is paid/running, initialized, reachable with the dedicated
   key, architecturally compatible, and a distinct machine.
4. Pin host keys, establish a short-path SSH control socket per host, and
   bootstrap only required packages.
5. Configure and verify UFW before deployment without locking out the
   orchestrator.
6. Build a locked release `cdk-mintd` with
   `sqlite,info-page,federation-bark,iroh,prometheus`. Hash it before upload.
7. Create a private operator directory on each host, upload mintd and the
   operator helper, initialize that host's Iroh identity, and write
   mode-`0600` configuration locally on the host.
8. Create the setup on member 1. Transfer each opaque invitation only to its
   intended member. Join all remaining members.
9. Propose, review, and approve the same proposal hash independently on every
   member. Require complete enrollment and a certified manifest.
10. Stop the coordinator after certification and start every DKG participant
    concurrently. Require identical proposal, manifest, transcript, final
    config, and public-config hashes.
11. Activate on every host and start all runtimes. Require Iroh-only public and
    federation URLs, healthy BDK Signet chain correlation, and Bark readiness.
12. Migrate one member at a time to systemd. Require ready and converged status
    before moving to the next member. Remove temporary tmux sessions afterward.
13. Run the mintd-only acceptance checks below and write a redacted report.

## Acceptance checks

Require all of the following:

- the number of distinct machine identities equals the member count;
- every systemd instance is enabled and active;
- every member passes `runtime-status` with ready, converged, zero operation
  lag, and no digest, payment-observation, or invalid-operation alerts;
- every member rejects its configured loopback HTTP listener probe;
- every public mint URL and federation API URL starts with `iroh://`;
- every member has the same finalized public-config hash;
- the BDK chain source reports `health=healthy` on the intended network;
- Bark reports `readiness=ready` on the intended network;
- UFW exposes only the approved SSH rule over IPv4 and IPv6;
- the deployed binary directory contains only `cdk-mintd`;
- no client CLI, TUI, wallet-smoke state, or tmux session remains;
- a rolling restart reconverges before the next member is touched.

## Handoff

Report the federation ID, all public Iroh URLs, member and threshold counts,
network/backends, public-config and mintd hashes, systemd/UFW state, readiness,
remaining funding or renewal requirements, and the redacted report path.
Never include private configuration, seeds, invitation contents, private keys,
account PII, or server addresses in repository-tracked output.
