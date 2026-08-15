# Policy evaluation failures

Severity: warning, escalating to critical when broad or sustained. Owner: policy/security on-call. Evaluation timeout, unavailable state, and internal failure must continue to deny access rather than bypass policy.

## Triage

1. Identify affected services and regions, then separate timeout, unavailable-policy, invalid-context, and evaluator-error outcomes.
2. Check active policy versions, authorization epochs, compiler diagnostics, evaluator saturation, and recent policy activations.
3. Confirm all affected public operations are failing closed and no service/operator bypass was implicitly enabled.

## Containment

1. Halt further policy promotions in the affected environment.
2. If a newly activated policy caused the incident, use the normal audited rollback to the last validated version.
3. Scale or restart unhealthy evaluators only after preserving safe diagnostics and correlation identifiers.

## Recovery

1. Re-run policy validation and example tests against the collection schema.
2. Confirm point reads, indexed queries, replication, live delivery, and edge SDK calls produce identical decisions.
3. Verify the authorization epoch and invalidation stream converge before declaring recovery.
4. Never mitigate by enabling a public bypass or including protected document bodies in diagnostics.
