# Tenant-isolation signal

Severity: critical/P0. Owner: security incident commander. Any impossible cross-project or cross-environment observation is presumed to be a confidentiality or integrity incident until disproved.

## Triage

1. Page security, storage, and the owning service teams immediately; record service, region, release, and alert time.
2. Use tenant IDs and correlation identifiers to establish scope. Do not place actor data, tokens, secrets, or document bodies in the incident channel.
3. Determine whether the signal came from key decoding, trusted-scope mismatch, cursor/checkpoint validation, cache ownership, object storage, or worker isolation.

## Containment

1. Remove the implicated service instances and release from traffic.
2. Suspend only the affected tenant paths when scope is proven; otherwise isolate the region. Keep immutable audit and storage evidence.
3. Revoke exposed credentials and sessions based on evidence. Do not delete or compact relevant records.

## Recovery

1. Reproduce the boundary violation with sanitized fixtures and identify the failed invariant.
2. Patch the root cause and run tenant-boundary, key-codec, gateway, storage, policy, and worker-isolation suites.
3. Restore traffic in stages while watching the isolation counter and audit stream.
4. Complete disclosure, credential rotation, and forensic retention steps under the security response policy before closure.
