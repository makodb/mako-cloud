# Storage contract failure

Severity: critical. Owner: storage on-call. The affected data plane must remain unready while required ordered scans, snapshots, atomic writes, conditional writes, or configured durability cannot be proven.

## Triage

1. Identify the affected `service` and `region` from `MakoStorageContractFailed`; confirm the readiness failure and its safe diagnostic.
2. Check adapter health latency, the advertised capability set, strongest acknowledged durability, and recent storage I/O or restart events.
3. Compare the last acknowledged commit high water with the adapter instance being checked. Do not infer that an empty or newly opened database is current.

## Containment

1. Keep the instance out of readiness and drain new data-plane traffic from it.
2. Stop automated failover if the destination has not independently proved both the adapter contract and possession of acknowledged state.
3. Do not switch to a local RocksDB directory, weaken durability, or disable capability checks to restore availability.

## Recovery

1. Repair connectivity, credentials, disk state, or the adapter implementation without changing the semantic contract.
2. Run storage readiness and the adapter conformance suite against the exact candidate backend.
3. Restore traffic only after readiness is continuously healthy and the acknowledged commit high water is present.
4. Escalate to incident command if acknowledged data is absent, durability cannot be verified, or multiple regions fail simultaneously.
