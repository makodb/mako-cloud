# Public beta environment runbook

This runbook covers VM 124 at `130.245.173.11` for
`cloud-test.makodb.com`. It is a single-VM, single-region beta: there is no HA,
automatic failover, or availability guarantee. A VM, host, data-disk, or network
failure can make the entire beta unavailable. Keep unrestricted public admission
disabled whenever evidence is incomplete or a critical alert is active.

## Access and first checks

Use the generated inventory and pinned host key:

```bash
ssh -o UserKnownHostsFile=.local/ansible/public-beta-known-hosts \
  mako-admin@130.245.173.11
```

Direct root SSH and password authentication are disabled. Recovery is through
the Proxmox console. On the guest, inspect `mako-release-operation inspect`, the
four Mako system services, the rootless `mako-dependencies.target`, the two
checkpoint timers, and `/var/lib/mako-health/health.json`. Prometheus and
Grafana are loopback-only; use an SSH tunnel when interactive access is needed.

## Developer console sign-in

The preview console uses the same-origin management API and the control plane's
short-lived signed developer sessions. It does not embed a shared credential or
expose a session-issuance endpoint publicly. From this protected workspace,
build or reuse the exact release utility and issue a session into a mode-0600
file:

```bash
mkdir -p -m 0700 .local/console-sessions
target/release/mako-control-session \
  --secret-file .local/public-beta-secrets/internal-auth \
  --output .local/console-sessions/developer.jwt \
  --issuer https://cloud-test.makodb.com/control-identity \
  --identity-id dev_publicbeta \
  --email msmummy@gmail.com \
  --display-name "Mako Public Beta Developer" \
  --ttl-seconds 3600
```

Open `https://cloud-test.makodb.com`, paste the file contents into the
short-lived developer session field, and sign in. The browser keeps the bearer
credential in tab-scoped session storage; closing the tab or signing out removes
it. The control plane verifies its signature, issuer, audience, status, and
expiry on every management request. Delete the token file after transferring it
and issue a new file after expiry. A production identity-provider adapter is
still required before offering self-service developer login.

## Deploy or converge

Build and validate an immutable release, update `mako_release_digest`, and run
the pinned Ansible playbook. Leave Caddy disabled. The normal deployment path
must not recreate the VM, replace either disk, delete a release, or alter DNS.
Use `mako-release-operation upgrade DIGEST --confirm=UPGRADE_RELEASE:DIGEST`
for a running stateful environment. Require verified checkpoints, active volume
markers, HTTP 200 from all four private readiness endpoints, active backup
timers, and the Proxmox admission-stop service after the operation.

Stage operator-password releases in restricted `pre_gate`: keep password auth disabled, retain only
explicit reason-bound break-glass rollback access, run the protected loopback bootstrap plan/apply,
then enable password auth and complete one browser sign-in without sharing the password. Disable
routine bearer acceptance and remove the prior local token only after that succeeds. Follow the
[operator password-authentication runbook](operator-password-authentication.md), and require a new
release/plan/blocker-bound public-preview approval before reopening admission.

Initial hosted fixture creation must use a real persistent developer that completed email
verification and operator approval. Record only its non-secret current identity binding in
`.local/qualification/public-beta-developer.json` with `schemaVersion`, `developerIdentityId`,
`email`, `displayName`, and `authorizationEpoch`. The qualification token helpers refuse missing,
malformed, stale-epoch, wait-listed, rejected, or disabled identities; do not restore the legacy
stateless `dev_publicbeta` bootstrap.

After that isolated fixture exists, exact-release streaming and benchmark requalification may use
`--reuse-fixture true`. This mode validates and reuses the protected project, environment,
collection, public credential, application user, and deployed test function in
`.local/qualification/public-beta-fixture.json`. It does not mint developer authority, approve a
wait-list applicant, or depend on a particular person's email. Pair it with the exact-release
hosted security qualification, which covers the management control plane independently.

Before live work, run `npm run validate:public-beta-local`. It only checks local
assets and fixtures. It never contacts Proxmox, applies the VM plan, enables
admission, or executes teardown. Retained public-beta evidence is in
`docs/evidence/`; release-operation records are in
`/var/lib/mako-release-operations/` on the guest.

## Risk-accepted public preview

This mode exposes an explicitly unqualified preview; it does not pass or bypass
the qualified-beta gate. After producing exact-release qualification evidence,
create and review the approval contract:

```sh
npm run public-beta:preview-approval -- plan --operator OPERATOR_IDENTITY --output .local/qualification/public-preview-plan.json
npm run public-beta:preview-approval -- approve --plan .local/qualification/public-preview-plan.json --confirm 'EXACT_CONFIRMATION_FROM_PLAN' --output .local/qualification/public-preview-approval.json
```

Converge with `mako_public_admission_mode: risk_accepted_preview` and the exact
approval path, blocker digest, and all eight non-waivable safeguards asserted.
Convergence starts Caddy in `pre_gate`; the guard checks the selected release,
readiness, backup freshness, trusted TLS/HSTS, persistent acceptance, and immutable
bindings before it atomically selects the preview configuration. Its one-minute
timer falls back to `pre_gate` on binding or safeguard drift. Inspect
`/var/lib/mako-public-preview/last-guard.json`; after a drift fallback, issue a new
acceptance before reactivation. To pause manually without stopping private
services or deleting wait-list state, run:

```sh
sudo /usr/local/sbin/mako-public-preview-admission pause
```

Manual pause invalidates the installed acceptance so an accidental `activate`
cannot reopen it; create a fresh acceptance to resume. Only the full release gate may select
`approved_beta`. The independent Proxmox emergency stop remains authoritative
in every mode.

## Certificate or renewal failure

Disable Caddy and apply the Proxmox admission stop first. Do not expose a
plaintext application fallback. Validate DNS A/AAAA state, ACME reachability,
the configured contact, staging issuance, production chain, hostname, expiry,
and renewal before re-enabling restricted ingress. HSTS remains off until a
trusted production certificate and external route checks pass. Certificate work
is blocked while the ACME contact or explicit pre-gate approval is missing.

## Capacity and saturation

Review the public-beta operations, production RocksDB, saturation, latency, and
error dashboards. Preserve at least the warning and critical filesystem
reserves from `group_vars/public_beta.yml`. If CPU, memory, network, queue, or
disk pressure persists, stop admission, retain the measurement window, and
either reduce qualification load or make a reviewed VM sizing change. Sizing a
single VM does not create HA.

## Backup and restore

The data-plane and control-plane timers create signed checkpoints every five
minutes and remotely reverify them on the Proxmox-host destination. A backup is
successful only when `mako_storage_backup_remote_verified` is 1 and age remains
below 900 seconds. Restore only to a new empty offline path with service-owned
credentials. Verify the signature, digests, format, tenant inventory, and
acknowledged high water; promotion requires explicit confirmation. Never
overwrite the only live path or substitute a VM snapshot for a Mako checkpoint.

## Service or dependency failure

Keep ingress off. Inspect the unit journal and request correlations without
logging credentials or document bodies. Restart only the failed system or
rootless-user unit. Do not clear a RocksDB lock by deleting files. If readiness
does not recover within the runbook objective, use release rollback or restore
to an empty offline target. Require all Mako, dependency, storage, backup, and
monitoring checks before ending the incident.

## Release rollback

Run `mako-release-operation inspect`, confirm Caddy is inactive, and identify the
recorded last-known-good digest. Then run:

```bash
sudo mako-release-operation rollback DIGEST \
  --confirm=ROLLBACK_RELEASE:DIGEST
```

The command checkpoints, fences all four non-empty paths, verifies format
compatibility, selects the immutable release, reactivates with the retained
compatible storage utility, and waits for readiness. Failure leaves ingress and
backup timers stopped for inspection. Retain the operation JSON from
`/var/lib/mako-release-operations/` and the tenant-boundary and audit results.
Operator keyspaces are additive and must remain untouched. Verify the selected release's expected
auth method, entitlement epoch, password recovery invalidation, and bounded break-glass rollback
before admission resumes.

## Suspected compromise

Apply the emergency admission stop, preserve logs and immutable evidence, stop
affected services, and revoke or rotate application credentials, internal
authentication, object-store credentials (loaded by both the control plane and
the data plane, which stores application objects with them), backup keys,
runtime-state keys, and certificates according to scope. Signing-key recovery is a safe roll-forward;
never reactivate suspected or retired private material. Do not destroy the VM or
backups until evidence retention and disclosure decisions are recorded.

## Emergency public-admission stop

Run `scripts/proxmox/public-beta-admission-stop.sh` from the workspace. It must
disable Caddy and enforce the VM 124 TCP 80/443 bridge drop while retaining SSH,
the VM, both disks, releases, evidence, and backups. Verify the host service and
nftables table are active and that external 80/443 no longer connect.

## Confirmed teardown

Teardown is separate from convergence and begins with the admission stop. Run
the teardown planner only; review its exact inventory of VM configuration,
OS/data disks, off-VM backups, credentials, certificate state, DNS consequences,
and retained evidence. Capture a final verified backup and evidence bundle.
Deletion, credential revocation, certificate handling, DNS changes, backup
disposal, and VM/disk removal each require the planner's typed confirmation.
Never execute teardown from an ordinary deploy, repair, or Ansible convergence.
