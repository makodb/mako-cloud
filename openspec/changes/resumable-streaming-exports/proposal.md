## Why

Collection exports currently buffer the entire result and fail above a 512 MiB object limit. Large migrations need consistent, resumable exports whose memory and transfer requests stay bounded regardless of collection size.

## What Changes

- Persist bounded immutable export parts and cursor progress between worker passes and retries.
- Publish version 2 manifests for multipart exports, with ordered part digests and a whole-output digest; retain version 1 downloads for single-part and existing exports.
- Resume from the same live internal snapshot, renewing its idle lease on page reads. If the snapshot disappears, restart one coherent generation and never combine snapshots.
- Extend authenticated artifact downloads and the installed CLI to verify and resume individual parts without exposing incomplete exports.
- Preserve cancellation, expiry cleanup, audit, tenant isolation, and resource safeguards.

## Capabilities

### New Capabilities

### Modified Capabilities

- `cloud/developer-data-explorer`: consistent multipart export, durable progress, safe restart, bounded authorized downloads.
- `cloud/developer-cli`: resumable multipart downloads with per-part and whole-output verification.

## Impact

Control-plane data jobs, internal export snapshots, API manifest and download contract, generated API types, management consumers and CLI. No source document writes or schema migration. A coordinated cloud release and installed CLI update are required for multipart exports; legacy clients must fail explicitly for multipart rather than save incomplete output.
