# Developer data workspace incident runbook

1. Identify the project/environment, request ID, operation, mode, and safe grant/target
   fingerprints from audit. Never request a capability, document body, import file, exported data,
   public-key value, raw reason, or user email.
2. Disable only the affected rollout gate: workspace, explorer administration, data jobs, sync
   detail, or restore. Legacy collection/user/function/credential/observability routes remain the
   rollback path.
3. For suspected stale or stolen explorer authority, advance the affected developer epoch. For
   collection, policy, schema, index, project, environment, or application-user lifecycle drift,
   advance all tenant explorer epochs. Confirm the old nonce now fails and produces a denied audit.
4. For import/export incidents, cancel future work, preserve the job ID and safe manifest/digest,
   and verify exact committed/failed/skipped/exported counts. Do not claim cancellation rolled back
   committed import rows. Keep downloads disabled until the complete artifact digest verifies.
5. For object-store outages, leave jobs retryable and allow idempotent retention cleanup to resume.
   Never publish a partial export or bypass the immutable tenant-bound address.
6. For restore incidents, keep the target inaccessible. Promotion and overwrite are prohibited for
   developer requests. Escalate verification/isolation failure through the operator recovery and
   production RocksDB runbooks.
7. Before re-enabling, verify audit continuity/redaction, bounded labels, cross-tenant denial,
   capability expiry/revocation, HTTPS allowlists, browser-storage absence, and the affected quota.
