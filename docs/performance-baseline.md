# Performance baseline

This baseline covers the required write, RxDB replication, authorization,
index, control-plane, and edge-supervisor paths. The machine-readable source is
[`performance-baseline.json`](performance-baseline.json), and
`npm run validate:performance` checks its shape and coverage.

## Measured results

| Path | Timed operation | p50 | p95 | Throughput |
| --- | --- | ---: | ---: | ---: |
| Write | Validate, synchronously commit one document, publish high water | 3.249 ms | 3.778 ms | 304 writes/s |
| Pull | Initial pull returning 100 visible documents | 7.158 ms | 9.750 ms | 140 pulls/s |
| Hidden-change scan | Scan 100 denied changes and advance the opaque checkpoint | 6.782 ms | 7.799 ms | 160 scans/s |
| Push conflict | Allocate/finalize a stale push and return the readable master | 3.823 ms | 4.861 ms | 249 conflicts/s |
| Live fan-out | Deliver one committed change to 25 sessions | 8.036 ms | 13.296 ms | 2,936 deliveries/s |
| Auth | Verify Ed25519 JWT, tenant, active session, and authorization epochs | 0.090 ms | 0.098 ms | 10,891 verifications/s |
| Policy | Evaluate three compiled read rules | 0.0010 ms | 0.0014 ms | 888,636 evaluations/s |
| Index build | Create, backfill 100 documents, catch up, fence, and activate | 38.660 ms | 49.453 ms | 25 builds/s |
| Control plane | Authorize an environment read and verify stored resource scope | 0.0052 ms | 0.0155 ms | 161,592 authorizations/s |
| Edge cold | Load a deployment, start an in-process worker, and invoke once | 0.0045 ms | 0.0073 ms | 173,213 paths/s |
| Edge warm | Invoke an already loaded in-process worker | 0.0014 ms | 0.0015 ms | 710,106 invocations/s |

Each row contains 25 release-profile samples. Storage-backed paths use the
local RocksDB optimistic-transaction adapter with `Sync` durability. Pull, hidden scan,
and index build use 100 seeded documents. Live throughput counts each of the 25
subscriber deliveries as one operation; its latency is the time to deliver to
all subscribers sequentially in the harness.

## Scope and interpretation

The measurements are single-process component baselines. They exclude HTTP
serialization, network latency, multi-node coordination, and distributed-store
behavior. Dataset creation and fixture setup occur before each timed sample.

The edge rows measure the real regional supervisor lifecycle through its public
worker boundary with an in-process worker factory. They deliberately do not
claim OCI image startup or Deno user-code latency. The pinned runtime's actual
container compatibility and adversarial behavior are covered separately by the
edge security qualification. Hosted cold-start thresholds must be set from the
production runtime adapter and deployment environment, not from these two
supervisor-overhead numbers.

This run used local RocksDB volumes on persistent Btrfs and:

- Linux 7.0.14-5-pve on x86_64
- Rust 1.97.1 and Cargo 1.97.1
- Node.js 24.15.0
- 25 samples, 100-document datasets, and 25 live subscribers

These values are an observed baseline, not release thresholds. Release gates
must select budgets for the target hardware, traffic mix, storage adapter, and
runtime deployment.

## Reproduce

Run the checked harness from the repository root:

```bash
npm run benchmark
```

The default output is `.playwright-tmp/performance-benchmark.json`. To retain a
report elsewhere or change the bounded workload:

```bash
MAKO_BENCH_OUTPUT=/tmp/mako-performance.json \
MAKO_BENCH_ITERATIONS=50 \
MAKO_BENCH_DATASET_DOCUMENTS=250 \
MAKO_BENCH_LIVE_SUBSCRIBERS=50 \
npm run benchmark
```

The runner always uses Cargo's release profile, a project-local Rust temporary
directory, disabled incremental compilation, and the locked dependency graph.
