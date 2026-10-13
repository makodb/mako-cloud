## Context

See proposal.md. Internal data-job snapshots already renew a five-minute idle lease on active reads. Explorer grants have a different, absolute lifetime and will not be used for bulk export. Object storage supports bounded immutable objects; HTTP responses currently buffer one object.

## Goals / Non-Goals

Goals: collection exports larger than 512 MiB with bounded payload memory, durable progress, consistent snapshots, verified resumable downloads and safe artifact retirement.
Non-goals: remove import size limits, weaken authorization, change source application data, or make a process-local RocksDB snapshot survive a data-plane restart.

## Decisions

- Export workers persist ordered parts, cursor, snapshot identity, schema version and page number. A pass writes a part targeting 16 MiB (at most one bounded RPC page over target, capped at 20 MiB), or yields after a bounded page count. Persist the upload intent before storing each immutable part, so cancellation/expiry can clean even interrupted uploads. Serialize worker passes within a service instance.
- Retry the same cursor/page under stable generation request identities. A lost snapshot returns a typed checkpoint-expired error. Delete the abandoned generation's recorded parts, reset progress, and open a new generation. Never publish parts from mixed snapshots. Successful progress resets consecutive worker failures.
- On completion, read parts one at a time to compute the existing whole-NDJSON SHA-256 contract, checking part digests and counts. Finalization can restart over durable parts without needing the snapshot. A bounded manifest holds at most 4096 parts; the payload bound is independent of collection size, with a separate explicit manifest-capacity guard.
- Single-part exports retain format 1 and the existing whole-artifact endpoint. Multipart format 2 adds ordered part descriptors. The existing authenticated download grant binds the job, actor, tenant and final digest. A part query selects only a committed manifest member. Missing part selection for format 2 fails explicitly, preventing legacy clients from accepting an incomplete file.
- The CLI appends verified parts into a protected .part file with a manifest identity sidecar, renewing grants between parts. Resume verifies the existing prefix, truncates an incomplete or corrupt suffix, and downloads the remaining parts. Verify total count/bytes/digest before atomic rename; stdout uses a protected temporary file and emits only after verification. A process lock prevents concurrent writers. Old format 1 remains supported. The console must support multipart or present an explicit CLI instruction before creating a download.

## Risks / Trade-offs

- Final hashing reads stored parts once more → bounded memory and unchanged external digest semantics; retry finalization safely over durable objects.
- Snapshot loss restarts collection capture → report progress reset; pin only while active, preserve one-snapshot integrity, bounded failure/restart policy.
- Manifest and per-pass work remain bounded → explicit capacity errors, cleanup and no partial publication.
- Cancellation races with object upload → durable intent plus serialized worker; expiry cleans intent and committed parts idempotently.
- An existing older control-plane binary cannot parse new private fields → after new-format records are created, rollback requires a compatible release (even terminal records retain their shape); never restore an old data snapshot. Keep prior source/build evidence; deploy all services coherently.

## Migration Plan

Run fault-injection worker and download tests plus API generation, lint, builds and real service checks. Deploy a verified immutable release, update the installed CLI, then create a new source export job because the former failed job is terminal. Download and verify the collection above 512 MiB with an interrupted-download resume test. Keep source writers fenced and destination head unpublished until full migration parity.
