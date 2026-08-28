# Control-plane SQLite authority

Mako Cloud stores control-plane identity and management state in one server-side SQLite database. This includes developer credentials and lifecycle, verification and recovery, wait-list decisions, operator entitlements and sessions, teams, projects, provisioning, control audit, idempotency, mail outbox, function metadata, and operator projections. The data plane continues to own application users, tenant credentials and signing keys, documents, policies, indexes, and RxDB synchronization state in RocksDB.

The adapter preserves the existing opaque byte-key/value encoding in `mako_kv(key BLOB PRIMARY KEY, value BLOB) WITHOUT ROWID`. This makes migration byte-exact and keeps repository and API behavior stable. Metadata records a database identity and format version. Every connection enables WAL, full synchronous durability, foreign keys, disabled trusted schema, a bounded busy timeout, and bounded automatic checkpointing. One process holds the configured external lock.

Production startup requires an existing database. A blank fallback is not created. Startup refuses unexpected application identity, missing/mismatched deployment identity, older or newer schema, symlinked/unsafe paths, integrity failure, critical capacity pressure, or an incomplete migration. Control readiness depends on SQLite, not tenant RocksDB. Tenant-data operations still fail with their scoped dependency error when the data plane is unavailable.

Migration is offline from a stopped, fenced RocksDB checkpoint. `mako-control-storage-ops` inventories the entire keyspace in byte order using framed key/value lengths and BLAKE3, copies into a temporary SQLite database, verifies the same count/checksum/prefix inventory and SQLite integrity, synchronizes it, and atomically publishes. The protected receipt binds source checkpoint, release, configuration, identities, formats, counts, checksums, and timestamps. An existing live target, unfenced or empty source, incomplete target, or mismatched result is refused.

SQLite backup uses the online backup API into an offline database, verifies identity/schema/integrity/inventory/high water and SHA-256, authenticates the manifest, publishes atomically, copies off the VM, and verifies again. Restore requires a nonexistent offline target, matching identity/release, permitted age, valid manifest/digest/inventory/high water, and explicit no-overwrite promotion.

This removes a shared storage-engine dependency. It does not make the single VM or disk highly available.
