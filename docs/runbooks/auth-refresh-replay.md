# Authentication refresh replay

Severity: critical. Owner: identity/security on-call. The identity service should revoke the full token family when a rotated refresh credential is replayed.

## Triage

1. Scope the alert to project/environment and inspect sanitized audit outcomes, affected family counts, source network class, and correlation identifiers.
2. Confirm the replay path revoked the family and gateways received the ordered revocation event within the freshness bound.
3. Determine whether the signal is an isolated client race within the configured grace window or credential theft; never retrieve or copy the raw refresh value.

## Containment

1. Revoke affected sessions or the application user if family revocation did not converge.
2. For a broad campaign, apply the auth endpoint throttle, suspend affected credentials, and engage the security incident process.
3. Preserve append-only auth audit records and access-controlled request traces.

## Recovery

1. Verify fresh sign-in succeeds while every token in the compromised family remains rejected.
2. Confirm revocation-cache freshness in every gateway region.
3. Review client refresh concurrency and grace configuration before changing it; do not expand grace to hide replay signals.
4. Rotate project signing keys only if evidence shows key compromise, not merely refresh-token theft.
