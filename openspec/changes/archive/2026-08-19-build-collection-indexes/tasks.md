## 1. Propagation

- [x] 1.1 Add `InstallIndex` and `InspectIndex` to the internal-RPC identity-admin contract with their input payloads.
- [x] 1.2 Handle both in the data plane under the existing collection-management permission.
- [x] 1.3 Propagate on index creation in the control plane before reporting the result.

## 2. Building

- [x] 2.1 Record the index definition in the data plane idempotently, so a retried propagation is not an error.
- [x] 2.2 Drive backfill in bounded resumable pages, catch up to the committed high water, and activate.
- [x] 2.3 Advance an unfinished build when the index is read, so a build that ran out of page budget converges.

## 3. Reporting

- [x] 3.1 Report the data plane's state from `getCollectionIndex`.
- [x] 3.2 Report it from `listCollectionIndexes` too, so the two cannot disagree.

## 4. Coverage

- [x] 4.1 Add an end-to-end test that creates an index through the management API and queries through it as an application user.
- [x] 4.2 Record the new scenario in the requirements traceability matrix.
