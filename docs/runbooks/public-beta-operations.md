# Public beta operations alerts

These alerts describe the private `cloud-test.makodb.com` beta VM. Keep public
admission disabled while a critical alert is active. Access Prometheus and
Grafana only through the documented SSH management path; their listeners remain
bound to loopback.

For a service or Podman dependency alert, record the release digest and request
correlation, inspect the relevant system or rootless-user journal, and restart
only the failed unit. Require all four `/readyz` endpoints before considering
recovery complete. Repeated restarts require rollback or an offline restore;
do not bypass readiness.

For operator authentication alerts, keep admission in `pre_gate`, use only aggregate counters, and
follow the [operator password-authentication runbook](operator-password-authentication.md). Any
lingering break-glass enablement or failed protected bootstrap requires review before admission.

For filesystem, RocksDB, backup, or recovery alerts, stop admission and writes,
preserve the sole live paths, verify the newest signed off-VM checkpoint, and
follow the production RocksDB recovery runbook. Never create an empty live path
or use an in-memory fallback.

For certificate alerts, leave Caddy admission off until staging and production
issuance, hostname validation, expiry telemetry, and renewal have all passed.
For latency, HTTP errors, audit failures, or host saturation, retain the alert
window, service logs, and resource graphs; audit failure is fail-closed and
requires operator review before admission resumes.

In `risk_accepted_preview`, the one-minute admission guard records sanitized
state in `/var/lib/mako-public-preview/last-guard.json`. Expiry, release or plan
drift, blocker drift, failed readiness, stale backups, or TLS/HSTS failure
atomically selects `pre_gate`; the same invalidated approval cannot reactivate
the preview. Fix the condition and issue a new exact-binding approval. Do not
manually replace the Caddy symlink or disable the timer.
