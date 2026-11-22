## Why

Four requirements say something the books contradict, and in each case the
code already follows the book.

- `cloud/developer-cli`, "Destructive commands are confirmed": the spec asks
  for "an interactive confirmation typed as the resource's name" and confirms
  a policy activation only "over an active one". The User Book says the
  resource's identifier is typed back, a policy activation is always
  confirmed, and the confirmed commands also include rolling back a
  deployment, restoring a backup and mutating a document. Its command
  reference additionally marks rotations and the cancelling of a data job as
  confirmed. The CLI types back the identifier and marks all of these.
- `identity/operator-authentication`, "Operator sessions are isolated,
  cookie-backed, and revocable": the spec scopes the cookie "to operator
  authentication and API paths". The Dev Book says the cookie is
  `Path=/v1`, and so does the code.
- `storage/control-plane-sqlite`, "RocksDB-to-SQLite migration is verified and
  restart safe": the spec says a later run "safely resumes or recreates only
  the temporary target" after an interruption. The Dev Book says an incomplete
  target is refused, and the migration tool refuses it until it has been
  inspected; only a completed migration is answered idempotently from its
  receipt.
- `cloud/developer-database-workspace`, "API and RxDB connection guidance":
  the spec says the workspace shows the "public project key" and puts it in
  the example. The User Book says the page shows the active public key id and
  that a key's value is shown once and cannot be read back.

## What Changes

Each of the four requirements is reworded to say what the book says. No
scenario is added, removed or renamed, so the traceability matrix does not
change. No code changes.

## Capabilities

### New Capabilities

### Modified Capabilities

- `cloud/developer-cli`: confirmation types back the resource's identifier,
  and the list of confirmed commands matches the User Book.
- `identity/operator-authentication`: the operator cookie's path is `/v1`.
- `storage/control-plane-sqlite`: an incomplete migration target is refused,
  not resumed.
- `cloud/developer-database-workspace`: connection guidance shows the public
  key id and a placeholder, never the key's value.

## Impact

- The four spec files named above.
