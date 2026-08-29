# Application file storage

Applications store files — images, uploads, attachments — next to their
documents, in **buckets** a developer creates per environment. Objects are
governed by the same policy language documents use, metered like every other
resource, and served by the data plane under
`/v1/projects/{projectId}/environments/{environmentId}/storage/{bucketId}/objects/{path}`.

## Buckets

A bucket declares:

- `access`: `policy` (every request is evaluated against the bucket's rules)
  or `public` (reads need no credential at all; writes are still evaluated).
- `maxObjectBytes`: the largest object it accepts, up to the platform ceiling
  of 16 MiB.
- `allowedContentTypes`: patterns such as `image/*` or `text/plain`; empty
  means any.
- `rules`: the document-policy language over the **object document** —
  `path`, `bucket`, `owner_id`, `content_type`, `size_bytes`, `created_at`,
  `updated_at` — with `new.*` on `create`/`update`, `old.*` on `read`,
  `update`, `delete`, and `identity.*`, `claims.*`, `request.*` as for
  collections. A bucket with no rules refuses every policy-governed request;
  rules that do not compile are refused at configuration time, so a bucket is
  never installed with a policy that cannot run.

Developers manage buckets through the management API
(`/v1/projects/{p}/environments/{e}/storage-buckets…`), the console's Storage
screen, and `mako storage buckets …`. The control plane holds no bucket
state: it authorizes the developer and forwards to the data plane, which is
the single source of truth for buckets, objects, and totals. Deleting a bucket
that still holds objects is refused unless the developer confirms their loss
(`deleteObjects=true`).

## Objects

- `PUT …/objects/{path}` with the object's `Content-Type` stores it; a new
  path is a `create`, an existing one an `update`. Paths are `/`-separated,
  1–512 bytes, and may not contain `.`/`..` segments, empty segments, or
  control characters — a path can never leave its bucket.
- `GET …/objects/{path}` serves the bytes with their content type and an
  `ETag` of the plaintext digest; a refusal sends no bytes.
- `DELETE …/objects/{path}` removes it.
- `GET …/objects?prefix=&limit=&cursor=` lists what the caller may read: each
  candidate is evaluated as a `read`, so a listing never names what a read
  would refuse.

Requests carry an application session (`Authorization: Bearer`) or, on the
loopback-only `/service/storage/…` routes, a service credential with the same
privileged-bypass audit as documents. Public buckets serve `GET` to anyone.

### Conditional uploads

An upload may carry a condition on what is stored at the path, so two
clients racing on one object cannot silently overwrite each other:

| Header | Meaning | If it does not hold |
| --- | --- | --- |
| `If-None-Match: *` | store only if the path is free (create-only) | `412 precondition_failed` |
| `If-Match: *` | store only if something is there (replace-only) | `412 precondition_failed` |
| `If-Match: "<etag>"` | store only if the stored object is exactly that version | `412 precondition_failed` |

The entity tag is the one a download returns: the plaintext digest in
quotes. Only a single strong tag is accepted — a list, a weak `W/"…"` tag,
or an `If-None-Match` other than `*` is refused with `400 invalid_request`
rather than ignored, so a caller that asked for a condition the platform
does not implement is never answered as if it had asked for none. Both
headers together are both checked.

Conditions are evaluated **after** the bucket's rules, so a caller the rules
refuse learns nothing about what is stored, and **atomically with the
commit**: the record they are checked against is the one the write is
conditioned on, so a concurrent change answers `409 conflict`, never a write
past a failed check. A refused upload writes nothing and leaves the stored
object and its `ETag` untouched. Nothing about a `DELETE` is conditional
today.

## At rest

Object metadata and per-bucket totals live in the environment's RocksDB
keyspace; bytes live in the platform object store under
`projects/{p}/environments/{e}/buckets/{bucket}/objects/{digest}.blob`,
**encrypted** with XChaCha20-Poly1305 under a key derived for the tenant from
the data plane's secret, bound to the bucket and path. The store addresses
ciphertext by its own digest, so its integrity check holds; the plaintext
digest is kept in the object's record and verified on every read. Re-uploading
a path mints a new address and retires the old bytes; the store itself stays
immutable. Objects are read and written whole (no streaming), which the 16 MiB
ceiling keeps affordable.

The data plane requires the object store's credentials (like the control
plane) and, in production, refuses readiness until the store answers.

## Metering and limits

- `object_storage_bytes` — a level: the environment's stored object bytes,
  from the totals every write keeps, sampled like `storage_bytes`.
- `object_egress_bytes_per_month` — a flow: bytes served by downloads,
  recorded before a byte leaves and cross-checked against the gateway's
  egress counter.

Every storage request is charged as an egress request; downloads are charged
their bytes. Plans include an allowance for each (free: 1 GiB stored, 5 GiB
egress, capped; pro: 50 GiB and 250 GiB, overage billed at $0.02/GiB-month and
$0.09/GiB). Caps reach the data plane on the installed quota policy: egress
through the gateway's `egress_bytes` window, stored bytes as a nominal window
whose limit the upload path compares against the running total. Refusals
answer 429 `quota_exceeded` with `retry: never`, or `rate_limited` with a delay
for the platform rate windows.

## Tests

`crates/mako-file-storage` unit tests cover encryption at rest, owner-only
access and anonymous refusal, atomic totals, path escapes, size and type
limits, the storage ceiling, forced removal, public buckets, filtered paging,
uncompilable rules, and per-tenant keys. `crates/mako-smoke/tests/file_storage.rs`
runs the whole flow against the real data and control planes with an
in-memory stand-in for the S3 store (`ObjectStoreStub`): bucket creation,
upload/download/list/delete under policy, refusals without bytes, path
escapes, conditional uploads (`If-None-Match: *` and `If-Match` against a
read `ETag`, including a stale one and a refused malformed condition), a
public bucket, developer listing and totals, and confirmed removal.
