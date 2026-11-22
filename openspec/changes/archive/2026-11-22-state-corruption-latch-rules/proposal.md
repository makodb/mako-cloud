## Why

`storage/production-rocksdb` says in one scenario that a corruption report
"fails readiness, stops accepting mutations, and directs operators to the
documented recovery procedure". Since commit e062ddf the RocksDB adapter does
that, and in doing so it settled details the scenario leaves open:

- which operations count (every one that RocksDB answers with a corruption
  error, the health probe included);
- how long the state lasts (until the process restarts; it never clears by
  itself);
- that reads stay open, so the volume can be inspected and preserved as the
  Dev Book's runbook asks;
- where an operator is directed (the readiness detail names the runbook);
- that an I/O error alone does not do any of this.

Behaviour that no requirement covers is written down here.

## What Changes

A new requirement, "A corruption signal stops mutations until restart", with
four scenarios. The existing requirement "Local storage health and capacity
are observable" and its scenario are left as they are, so the open change
`observable-data-plane-and-restore-verification`, which modifies that
requirement, is not affected.

The rule for I/O errors states what the code does today. The report asks the
whitepaper's author whether repeated I/O errors should also fail readiness;
an answer would change this requirement.

No code changes.

## Capabilities

### New Capabilities

### Modified Capabilities

- `storage/production-rocksdb`: what a corruption signal does to the service
  that met it is a requirement of its own.

## Impact

- `openspec/specs/storage/production-rocksdb/spec.md`
- The capability is not indexed by the traceability matrix, so no rows change.
