# Service configuration

Every service loads the same validated configuration model before opening listeners or storage. Defaults target local development. Set `MAKO_CONFIG_FILE` to an optional JSON file, then use environment variables for deployment-specific overrides; environment values always win.

The example at `config/mako.local.json.example` documents every JSON section for the data plane, and `config/mako.control.local.json.example` does the same for the control plane's SQLite sections. Unknown JSON fields, malformed addresses or URLs, invalid regions, empty paths, out-of-range limits, incomplete TLS pairs, and insecure production public URLs stop startup with a field-addressed error code.

## Environment overrides

- `MAKO_ENVIRONMENT`: `local`, `development`, `staging`, or `production`
- `MAKO_REGION`: lowercase region slug
- `MAKO_BIND_ADDR`, `MAKO_PUBLIC_URL`
- `MAKO_TLS_CERT_PATH`, `MAKO_TLS_KEY_PATH`
- `MAKO_ROCKSDB_PATH`
- `MAKO_ROCKSDB_MAX_BATCH_OPERATIONS`, `MAKO_ROCKSDB_MAX_SCAN_ITEMS`
- `MAKO_ROCKSDB_LOCK_TIMEOUT_SECONDS`, `MAKO_ROCKSDB_TRANSACTION_EXPIRATION_SECONDS`
- `MAKO_ROCKSDB_BACKUP_DESTINATION`, `MAKO_ROCKSDB_BACKUP_RETENTION_COUNT`
- `MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES`, `MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES`
- Control plane only: `MAKO_CONTROL_SQLITE_PATH`, `MAKO_CONTROL_SQLITE_LOCK_PATH`, `MAKO_CONTROL_SQLITE_IDENTITY`
- Control plane only: `MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE`, `MAKO_CONTROL_SQLITE_BACKUP_STAGING`, `MAKO_CONTROL_SQLITE_BACKUP_PUBLISH`, `MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE`, `MAKO_CONTROL_SQLITE_RESERVE_PATH`
- Control plane only: `MAKO_CONTROL_SQLITE_MAX_BATCH_OPERATIONS`, `MAKO_CONTROL_SQLITE_MAX_SCAN_ITEMS`, `MAKO_CONTROL_SQLITE_BUSY_TIMEOUT_SECONDS`, `MAKO_CONTROL_SQLITE_TRANSACTION_EXPIRATION_SECONDS`, `MAKO_CONTROL_SQLITE_SHUTDOWN_TIMEOUT_SECONDS`
- Control plane only: `MAKO_CONTROL_SQLITE_WAL_AUTOCHECKPOINT_PAGES`, `MAKO_CONTROL_SQLITE_MAX_WAL_BYTES`, `MAKO_CONTROL_SQLITE_INTEGRITY_INTERVAL_SECONDS`, `MAKO_CONTROL_SQLITE_BACKUP_RETENTION_COUNT`, and its warning/critical disk thresholds
- `MAKO_SMTP_ENDPOINT`, `MAKO_OBJECT_STORE_ENDPOINT`, `MAKO_RUNTIME_SUPERVISOR_ENDPOINT`, `MAKO_TELEMETRY_QUERY_ENDPOINT`, `MAKO_OTLP_ENDPOINT`
- `MAKO_MAX_REQUEST_BYTES`, `MAKO_SHUTDOWN_GRACE_SECONDS`
- `MAKO_INTERNAL_AUTH_SECRET_REF`
- `MAKO_OBJECT_STORE_ACCESS_KEY_REF`, `MAKO_OBJECT_STORE_SECRET_KEY_REF`

Relative paths are resolved from the service working directory only outside production. A production stateful service requires normalized absolute database and backup paths outside known ephemeral filesystems. Control SQLite live, lock, migration, backup, restore, and reserve paths are mutually non-overlapping and reject symlink components. Batch/scan limits and transaction timeouts are positive and bounded. The warning disk reserve must exceed the critical reserve, whose production minimum is 64 MiB. Durability and backend kind are not configurable: the control plane uses synchronous WAL SQLite, while data-plane, edge-gateway, and telemetry use synchronous RocksDB. A production control plane rejects legacy `storage.rocksdb_path`; the other services still require it.

## Secret references

Configuration accepts references, never inline secret values:

- `env:VARIABLE_NAME` reads an existing process environment variable.
- `file:/mounted/path` reads a UTF-8 secret file up to 64 KiB and removes one trailing newline.

Resolved values are redacted from `Debug`, `Display`, and startup summaries. Production startup requires an internal-auth secret reference and an HTTPS public URL. The production control plane additionally requires paired object-store access-key and secret-key references. Keep referenced environment variables and files out of source control.

## Startup diagnostics

Failures are emitted as a stable code, field path, and non-sensitive explanation, for example:

```text
configuration error CONFIG_INVALID_VALUE at server.bind_address: must be an IP address and port
```

Successful startup emits only service, deployment, listener, storage path, and whether TLS/internal and object-store authentication are configured. It never prints secret values.

Hosted developer registration is deny-by-default. Enabling
`developer_registration.enabled` requires bounded token/session lifetimes,
layered rate limits, password-work and pending-outbox limits, finite retention,
a protected mail-encryption secret, and a complete authenticated SMTP
relay/port/TLS mode/username/sender/password set. Production accepts only
verified TLS modes (`starttls` or wrapper TLS); missing mail material stops
startup. The public origin is the validated HTTPS server URL. See
[developer registration](developer-registration.md).

## Public-beta configuration

The VM renders one production JSON document per Mako service. All application,
telemetry, object-store, mail, and runtime-supervisor listeners remain on
loopback; Caddy is the only component permitted to terminate public HTTPS. The
public URL in every service document is exactly
`https://cloud-test.makodb.com`. The control plane owns one server-side SQLite
database. Data-plane, edge-gateway, and telemetry-query retain distinct RocksDB
paths. Secret
fields are `file:/run/credentials/...` references populated by systemd and are
never copied into the rendered JSON or Ansible logs.

Transactional developer mail and operator alerts both use the verified Resend
domain but remain separate delivery paths. The control plane loads its
authenticated SMTP password through its own systemd credential. Alertmanager
loads a protected `alert-smtp-password` credential, refers to it with
`smtp_auth_password_file`, requires STARTTLS on port 587, and never stores the
credential value in its rendered YAML or retained evidence. Application mail
readiness does not by itself prove operator-alert delivery.

`infra/ansible/group_vars/public_beta.yml` is the non-secret desired-state
selector. Its safe defaults are `mako_public_admission_mode: disabled`,
`mako_caddy_enabled: false`, no tester CIDRs, no admission approval, and no ACME
contact. Do not work around those defaults by editing a generated Caddyfile or
adding a plaintext listener. Certificate configuration requires an
operator-provided ACME contact and explicit approval for the restricted
pre-gate; HSTS is enabled only after the production chain, hostname, redirect,
route allowlist, and renewal checks pass.

Admission has four explicit values: `disabled`, `pre_gate`,
`risk_accepted_preview`, and `approved_beta`. Preview is not beta approval and
does not change any release-gate threshold. It requires a digest-bound approval
from `npm run public-beta:preview-approval` naming the exact operator, release,
plan, and blocker set. The acceptance has no calendar expiry and remains active
until an operator pauses it, the exact release or blocker binding changes, or a
non-waivable safeguard fails. Only incomplete SMTP delivery, latency targets,
and the 30-day capacity/cost evidence may be accepted. Trusted HTTPS with HSTS, the exact route allowlist,
application security, backup and recovery, zero acknowledged-write loss, zero
integrity failures, readiness, and the emergency stop are never waivable.
Convergence renders all four Caddy configurations; the admission guard selects
one atomically and falls back to `pre_gate`. Never hand-edit its symlink or a
rendered Caddyfile.

The rendered configuration and redaction result are retained in
`docs/evidence/public-beta-production-configuration.json`. Validate all static
assets with `npm run validate:public-beta-local` before convergence.
