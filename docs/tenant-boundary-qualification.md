# Tenant-boundary qualification

The qualification command is `npm run test:tenant-boundaries`. It runs the production boundary code with deterministic randomized inputs; failures retain Proptest's minimal reproducer and seed in the normal test output.

## Coverage

| Boundary | Property or invariant | Randomized cases per run |
| --- | --- | ---: |
| Key encoding | Arbitrary bytes round-trip without collisions; tenant and collection prefixes cannot escape or overlap | 1,792 |
| Storage | Memory and RocksDB adapters preserve tenant scan isolation through the shared conformance suite | deterministic conformance |
| Gateways | Identical reservation IDs and limits remain independent for arbitrary distinct project/environment pairs | 256 |
| Management APIs | Function records and range scans for arbitrary distinct project/environment pairs remain disjoint | 512 |
| Structured logs | A log event cannot be appended through a different arbitrary trusted tenant | 256 |
| Object storage | Typed bundle paths retain their owning tenant, reject cross-tenant reads, fuzz invalid digests, and preserve immutable writes | 1,024 |
| Edge workers | Identically named and versioned deployments resolve to distinct workers for arbitrary tenant pairs | 128 |

The randomized properties execute at least 3,968 generated cases per qualification run, in addition to deterministic unit, adapter-conformance, fault-injection, and restart tests in the selected crates.

## Latest qualification

On 2026-08-07, all selected suites passed locally:

- `mako-storage`: 52 unit, conformance, fault-injection, production startup,
  backup/restore, crash, CLI, and property tests
- `mako-gateway`: 10 tests
- `mako-control-plane`: 29 tests
- `mako-audit`: 21 library and threat-model tests
- `mako-object-store`: 3 tests
- `mako-edge-runtime`: 6 tests

The RocksDB result covers production key encoding, the semantic adapter,
empty-target restore, tenant-inventory verification, and acknowledged-high-water
recovery. The object-store result qualifies the typed boundary and deterministic
in-memory reference used by function bundle storage; qualification of a hosted
S3-compatible provider belongs in environment-specific release testing.
