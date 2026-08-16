# Distributed CDK Federation Runbook

Use this runbook for one independent `cdk-mintd` member per SSH server. Keep
provider purchase/payment flows separate from the federation ceremony.

## Contents

1. [Required inputs](#required-inputs)
2. [Safety gates](#safety-gates)
3. [Inspect the reference deployment](#inspect-the-reference-deployment)
4. [Build the inventory](#build-the-inventory)
5. [Pin SSH host keys](#pin-ssh-host-keys)
6. [Bootstrap hosts and UFW](#bootstrap-hosts-and-ufw)
7. [Build mintd](#build-mintd)
8. [Install independent operators](#install-independent-operators)
9. [Run invitation setup and DKG](#run-invitation-setup-and-dkg)
10. [Activate and verify runtime](#activate-and-verify-runtime)
11. [Install systemd](#install-systemd)
12. [Mintd-only audit](#mintd-only-audit)
13. [Upgrades and recovery](#upgrades-and-recovery)
14. [Failure playbook](#failure-playbook)
15. [Redacted report](#redacted-report)

## Required inputs

Collect these without writing secrets into the repository:

- exactly one SSH target per member, such as `user@host`;
- a dedicated readable private key path;
- a pinned known-hosts file;
- a safe relative remote root;
- member count and explicit signature threshold;
- target CPU architecture;
- Bitcoin network, normally `signet` for a drill;
- Esplora URL and stable chain-source identifier;
- Bark server URL and compressed Ark public key;
- CDK and Bark source revisions;
- expected systemd user and home directory.

For five members, distinguish the thresholds:

- signature threshold: normally 3;
- consensus/payment-observation threshold: normally 4.

Do not infer that a 3-of-5 signing threshold makes two simultaneous member
failures operationally safe for every protocol phase.

## Safety gates

Pause for explicit approval before:

- purchasing, renewing, or paying for hosts;
- changing account identity or billing information;
- replacing an active DKG ceremony;
- deleting or reinstalling a server;
- deleting operator state, encrypted backups, or private configs;
- changing the member set or threshold;
- opening inbound ports beyond the approved SSH rule;
- funding or spending from BDK or Bark.

Use read-only reconciliation after any ambiguous API or SSH response. Never
repeat a create, payment, DKG, or destructive request until current state is
known.

## Inspect the reference deployment

Read these files before adapting anything:

```bash
sed -n '1,380p' scripts/deploy_nocean_federation.sh
sed -n '1,760p' scripts/nocean_federation_control.sh
sed -n '1,940p' scripts/deploy_nocean_multi_operator_federation.sh
sed -n '1,700p' scripts/nocean_federation_operator.sh
```

If a live reference federation is available, inspect only safe fields:

- mintd version and binary hash;
- process count and RSS;
- disk usage;
- member readiness and convergence;
- BDK chain-source health;
- Bark readiness;
- loopback/public listener model;
- UFW policy;
- public federation config.

Never print runtime seeds, Bark secrets, activated private configs, encrypted
backup passwords, SSH private keys, or provider credentials.

The single-host scripts prove the protocol lifecycle, not failure-domain
independence. Require distinct machine identities for a distributed claim.

## Build the inventory

Keep the host list outside tracked files:

```bash
export OPERATOR_HOSTS_CSV='user@host1,user@host2,user@host3,user@host4,user@host5'
export SSH_IDENTITY_FILE='/secure/path/to/key'
export SSH_KNOWN_HOSTS_FILE='/secure/path/to/known_hosts'
export REMOTE_ROOT='cdk-federation-iroh'
```

For each host, verify:

```bash
ssh <options> user@host 'cloud-init status'
ssh <options> user@host 'uname -m'
ssh <options> user@host 'free -h'
ssh <options> user@host 'df -h /'
ssh <options> user@host 'sha256sum /etc/machine-id'
```

Hash `/etc/machine-id`; do not collect or publish its raw value. Require one
unique hash per member.

Small nodes are suitable only when binaries are built elsewhere. A measured
five-member reference used roughly 60 MiB RSS per member during DKG and
120 MiB per running member, but always remeasure the current revision and leave
headroom for the OS, SQLite, Iroh, Bark, and growth.

## Pin SSH host keys

Prefer provider-published Ed25519 fingerprints. Poll if a newly provisioned
host has not published them yet.

Scan without trusting:

```bash
ssh-keyscan -T 10 -t ed25519 host |
    ssh-keygen -lf -
```

Compare the fingerprint with the provider value through a separate authenticated
channel. Write the verified public key to a mode-`0600` temporary
known-hosts file. Use:

```text
-o BatchMode=yes
-o IdentitiesOnly=yes
-o StrictHostKeyChecking=yes
-o UserKnownHostsFile=<pinned-file>
```

Do not use `StrictHostKeyChecking=no` or blindly accept a changed host key.

### Multiplex deployment connections

UFW SSH rate limiting can reject the many short SSH/SCP connections used by a
deployment driver. Preserve the firewall rule and reuse one connection:

```bash
install -d -m 700 /tmp/cdf
export SSH_CONTROL_PATH='/tmp/cdf/%C'
```

Add:

```text
-o ControlMaster=auto
-o ControlPersist=600
-o ControlPath=/tmp/cdf/%C
```

Keep the socket path short. macOS Unix-domain socket limits can reject long
paths before any host is contacted.

Create and use multiplex sockets inside the same long-running deployment
process. Sockets created by a separate sandboxed process may not accept later
sessions.

## Bootstrap hosts and UFW

Wait for cloud-init. Install only runtime/deployment dependencies:

```bash
sudo apt-get update
sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y \
    --no-install-recommends ca-certificates curl jq openssl tmux ufw
```

Configure:

```bash
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw limit 22/tcp
sudo ufw --force enable
sudo ufw status verbose
```

Verify SSH survives before proceeding. Do not open 80/443 for Iroh-only
deployment. Keep mintd HTTP and Prometheus on loopback. Iroh N0 discovery and
relay operation work with outbound network access.

If deployment connections trigger `Connection refused`, wait for the short
rate-limit window and switch the driver to SSH multiplexing. Do not widen SSH
to unrestricted inbound access merely to accommodate automation.

## Build mintd

Use the pinned toolchain and lockfile. Build for the servers' architecture:

```bash
cargo build --locked --release -p cdk-mintd \
    --no-default-features \
    --features sqlite,info-page,federation-bdk,federation-bark,iroh,prometheus
```

The repository's Docker build pattern is preferred when the workstation
architecture differs from the servers. Mount the CDK checkout and the sibling
Bark checkout read-only where possible. Install build-only packages inside the
container, not on the VPSs.

Before upload:

```bash
sha256sum target/linux-x86_64/release/cdk-mintd
target/linux-x86_64/release/cdk-mintd --version
target/linux-x86_64/release/cdk-mintd federation setup create --help >/dev/null
target/linux-x86_64/release/cdk-mintd iroh init --help >/dev/null
```

Do not build or upload `cdk-cli` or `fed-cdk-tui` for a mintd-only server
deployment.

When using `scripts/deploy_nocean_multi_operator_federation.sh` as the
ceremony driver for independent hosts, set `MINTD_ONLY=1`. This keeps the
client wallet, TUI, and wallet-smoke state off every server; run paid
acceptance from a separate local client instead.

## Install independent operators

For member `N`, use a private directory such as:

```text
<remote-root>/operator-N/
├── bin/cdk-mintd
├── scripts/nocean_federation_operator.sh
├── work/
├── ceremonies/
├── operator.env
└── runtime-config.toml
```

Set the operator root, environment, runtime config, ceremony state, Iroh secret,
and activated private config to mode `0700` directories and `0600` files as
appropriate.

Initialize Iroh on the owning host:

```bash
bin/cdk-mintd --work-dir work iroh init
```

Require a canonical `iroh://<endpoint-id>` result. Never generate all Iroh
identities or runtime secrets centrally.

Use these core runtime values:

```toml
[info]
http_enabled = false
listen_host = "127.0.0.1"

[iroh]
enabled = true
discovery = "n0"

[prometheus]
enabled = true
address = "127.0.0.1"

[database]
engine = "sqlite"

[ln]
ln_backend = "federated-bark"

[onchain]
onchain_backend = "federated-bdk"

[federation]
enabled = true
```

Generate all omitted secret values on the member with `openssl rand`. Never
copy secret examples from this runbook.

## Run invitation setup and DKG

Use the ordinary setup transcript:

1. Member 1: `federation setup create`.
2. Member 1: start `federation setup serve`.
3. Transfer each `member-N.invite` through a mode-`0700` local staging
   directory and install it mode `0600` only on member N.
4. Members 2..N: `federation setup join`.
5. Member 1: require complete enrollment, then `federation setup propose`.
6. Every member: `federation setup review --json`.
7. Compare proposal hashes; every member must see the same value.
8. Every member: `federation setup approve --proposal-hash <hash>`.
9. Every member: require participant phase `certified`.
10. Stop the coordinator after certification.
11. Start `federation setup serve` concurrently on every member for DKG.
12. Poll bounded status until `dkg.finalized_ready == true`.

Do not serialize DKG participants. Keep all DKG services running concurrently.
On small one-vCPU hosts, status reads can take longer than on the reference
server; use bounded timeouts and avoid restarting a healthy ceremony solely
because a read is slow.

Collect and compare:

- proposal hash;
- manifest hash;
- DKG transcript hash;
- final config digest;
- public config hash.

Any mismatch is a hard stop. Do not activate.

## Activate and verify runtime

Stop setup services. Run `federation setup activate` locally on each member,
then start mintd with its own work directory and runtime config.

Require the public config to show:

- the expected member and threshold counts;
- only `iroh://` public mint and federation API URLs;
- equal public mint and federation API URLs for Iroh-only operation;
- the expected BDK and Bark networks;
- the accepted Bark Ark public key;
- the requested denomination set.

Use mintd-only readiness:

```bash
cd <operator-root>
timeout 180 bash scripts/nocean_federation_operator.sh runtime-status
timeout 60 bash scripts/nocean_federation_operator.sh http-listener-disabled
```

Require ready, converged, zero operation lag, no digest alerts, no payment
observation alerts, no invalid operation alerts, a healthy chain source, and
Bark readiness.

## Install systemd

Install the reviewed instance unit on every host. Verify a concrete instance,
not the bare template: `systemd-analyze verify` substitutes the literal
instance `i` for a bare `@.service` and can report a false missing-path
error.

```bash
sudo install -m 644 scripts/cdk-federation-mintd@.service \
    /etc/systemd/system/cdk-federation-mintd@.service
sudo systemctl daemon-reload
sudo systemd-analyze verify cdk-federation-mintd@1.service
```

Migrate one member at a time:

1. Stop that member's temporary tmux service.
2. Run `systemctl enable --now cdk-federation-mintd@N.service`.
3. Retry bounded `runtime-status` until the entire federation reconverges.
4. Expect brief Iroh request timeouts during restart; do not advance on them.
5. Kill the old operator tmux session only after convergence.
6. Move to the next member.

At every step, keep enough members online for consensus.

## Mintd-only audit

On every host, require:

```bash
sudo systemctl is-enabled cdk-federation-mintd@N.service
sudo systemctl is-active cdk-federation-mintd@N.service
find <operator-root>/bin -maxdepth 1 -type f -printf '%f\n'
find <operator-root> -type f \
    \( -name cdk-cli -o -name fed-cdk-tui -o -name '*wallet-smoke*' \)
tmux list-sessions
sudo ufw status numbered
```

The binary listing must contain only `cdk-mintd`; the client-artifact search
and tmux listing must be empty. UFW must expose only the approved SSH rule.

Scan preserved partial-upload directories too. If they contain client tools,
inventory them read-only and remove only confirmed unused artifacts unless the
user explicitly authorizes broader deletion.

## Upgrades and recovery

For a binary-only upgrade:

1. Build and hash the new mintd.
2. Upload it under a hash-qualified temporary name.
3. Verify the remote hash.
4. Replace one member binary.
5. Restart that member through systemd.
6. Require full convergence before continuing.

Do not regenerate runtime configs, Iroh identities, DKG state, or activated
private configs during an ordinary upgrade.

Before destructive recovery, inventory:

- systemd state and logs;
- runtime-config hash;
- public/private config presence;
- SQLite files and sizes;
- Iroh endpoint identity;
- current federation status;
- encrypted operator-local backup.

Preserve ambiguous partial state by renaming its exact directory. Never delete
a broad home, workspace, or remote root.

## Failure playbook

### Empty or timed-out create response

Reconcile the authoritative list before retrying. A timed-out request may have
succeeded remotely.

### SSH works manually but SCP later fails

Inspect UFW. If SSH uses `LIMIT IN`, rapid new connections may trigger the
rate window. Use same-process ControlMaster multiplexing with a short socket
path.

### Multiplex socket path is too long

Use `/tmp/cdf/%C`. Unix socket limits apply before network access.

### Existing operator directory blocks rerun

Inventory it. If it contains only an incomplete upload, preserve it under a
specific failed-upload name or remove exact unused files with approval. Never
blindly recurse over the remote root.

### DKG status reads are slow

Check that all setup services remain alive and connected. Wait within the
configured bound. Do not restart all members together.

### Runtime status reports request timeout after restart

Retry within a bounded loop. Advance only after ready and converged status
passes.

### systemd template verification reports `operator-i`

Install the template, reload systemd, and verify a concrete numbered instance.

### Provider places all hosts in one region

State clearly that separate VMs reduce process/host coupling but do not remove
provider, rack, network, or region failure domains.

## Redacted report

Record:

- result and UTC completion time;
- source revision and whether deployment scripts were dirty;
- mintd and public-config hashes;
- federation ID;
- member, signature-threshold, and consensus-threshold counts;
- public Iroh URLs;
- network, BDK source health, and Bark readiness;
- systemd enabled/active counts;
- UFW policy;
- mintd-only and HTTP-disabled checks;
- setup, DKG, activation, and rolling restart results;
- whether a paid functional wallet operation was intentionally skipped.

Exclude server IPs, account data, email, SSH material, private configs, seeds,
invitations, payment invoices, provider resource IDs, and local usernames or
absolute paths.
