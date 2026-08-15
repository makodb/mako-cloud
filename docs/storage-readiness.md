# Storage readiness contract

The data plane does not become ready merely because an adapter opened. `check_storage_readiness` collects its semantic capabilities and health result, compares its strongest acknowledgement to the configured durability requirement, and requires a positive restart-durability verification for WAL or sync operation.

The document engine requires point reads, lexicographically ordered bounded scans, stable snapshots, atomic batches, atomic conditional writes, and durable restart behavior. A missing claim is reported by name. Unhealthy/degraded health, a health-check error, weaker durability, or unverified durability independently makes readiness false.

`mako-data-plane` opens its configured local RocksDB path and runs this gate before it would start accepting traffic. Its safe result contains capability names, durability mode, health class, and retryability, but no vendor error text, storage keys, values, or document data. The deterministic in-memory adapter is intentionally rejected because it does not claim restart durability; the local RocksDB adapter passes after its health and durability checks.
