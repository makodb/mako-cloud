## Why

`cloud/developer-data-explorer`, "Conditional document mutations", says a
delete needs an idempotency key and the expected current revision. It does not
say what a delete's document content is, and until commit 45fad8e the data
plane refused every delete that sent none, which is what the console and the
CLI send. The fix settled the details:

- a delete may omit the content; it is then judged on the stored document's
  body, and the tombstone keeps that body;
- a delete that sends content is judged on what it sent;
- create and update require content;
- when the stored document no longer fits the active schema, the refusal says
  that and nothing about its fields.

## What Changes

"Conditional document mutations" states those rules and gains two scenarios:
"Delete is requested without document content" and "Create or update is
requested without document content". The existing scenarios are unchanged.

No code changes: the data plane has a test for each.

## Capabilities

### New Capabilities

### Modified Capabilities

- `cloud/developer-data-explorer`: what document content a delete, a create
  and an update carry.

## Impact

- `openspec/specs/cloud/developer-data-explorer/spec.md`
- The capability is not indexed by the traceability matrix, so no rows change.
