# Edge sandbox incident

Severity: critical. Owner: edge-runtime/security on-call. Signals include cross-project memory/environment access, secret leakage prevention, denied metadata egress, resource abuse, or work continuing beyond invocation lifetime.

## Triage

1. Identify region and bounded `reason`; correlate the invocation through access-controlled request/trace IDs without retrieving secret values.
2. Confirm the supervisor recycled only the compromised project worker and that neighboring tenants remained healthy.
3. Inspect immutable deployment version, runtime digest, configured limits, outbound policy, and secret-version selection.

## Containment

1. Disable or roll back the implicated function version and drain its project worker pool.
2. Block suspicious outbound destinations at the runtime boundary; never relax the egress policy to aid diagnosis.
3. Rotate only secrets that may have been exposed, and preserve redacted runtime/audit evidence.

## Recovery

1. Reproduce with a non-sensitive fixture against the exact pinned runtime image.
2. Run adversarial isolation, secret-redaction, egress, resource-exhaustion, crash, and post-lifetime-work tests.
3. Re-enable the deployment gradually and verify worker generations, recycle rate, invocation failures, and neighboring tenant health.
4. Escalate to platform security if isolation was bypassed, secret material reached output, or work survived worker termination.
