# RxDB client integration

`@mako-cloud/rxdb` is the supported application client for Mako Cloud. It connects a normal
RxDB collection to Mako's authenticated pull, push, and SSE endpoints; it does not expose the
underlying RocksDB-compatible storage interface.

The code below follows the browser-tested implementation in
[`examples/local-first`](../examples/local-first/README.md). The example's fake backend implements
the same public wire protocol so it can test offline behavior without a hosted environment.

## Install and configure

Install a supported RxDB 17 release and RxJS 7 alongside the Mako adapter:

```sh
npm install @mako-cloud/rxdb rxdb@17 rxjs@7
```

Create one normalized configuration per replicated collection. A public project key starts with
`mako_pk.` and is safe to ship in an application; it identifies the project but grants no access by
itself. Do not put a service credential in browser or mobile code.

```ts
import { normalizeMakoRxdbConfig } from "@mako-cloud/rxdb";
import { RXDB_VERSION } from "rxdb/plugins/utils";

const config = normalizeMakoRxdbConfig({
  endpoint: "https://api.example.mako.cloud",
  projectId: "prj_abcdefgh",
  environmentId: "env_abcdefgh",
  collectionId: "todos",
  schemaVersion: 1,
  publicProjectKey: "mako_pk.example",
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
  pullBatchSize: 100,
  pushBatchSize: 100,
});
```

Production endpoints must use HTTPS. Plain HTTP is accepted only for `localhost` and `127.0.0.1`.
The adapter rejects an unsupported RxDB major before starting replication.

The developer console's **API & Connect** page emits versioned template 1 from
`createMakoRxdbConnectTemplateV1`. The exact template is compiled in the local-first example on
every TypeScript qualification run. Its connection check submits only the public key ID,
collection/schema metadata, and RxDB version; it never sends a public-key secret or creates an
application-user session.

## Authenticate an application user

`MakoAuthClient` manages application-user access and refresh tokens. It never uses control-plane
developer sessions or service credentials.

```ts
import { MakoAuthClient } from "@mako-cloud/rxdb";

const auth = new MakoAuthClient(config, {
  persistence: encryptedSessionPersistence,
});

await auth.restoreSession();
if (auth.currentSession() === null) {
  await auth.signInWithPassword(email, password);
}
```

Implement `AuthSessionPersistence` using the platform's protected credential storage. The public
`MakoUserSession` intentionally omits the refresh token. `validAccessToken()` refreshes an expiring
token, deduplicates concurrent refresh calls, and throws `MakoAuthenticationRequiredError` after a
revoked or unavailable refresh session has been cleared.

## Connect an RxDB collection

Use `MakoCheckpoint` as the replication checkpoint type. Its `token` is an opaque, server-signed
value; never inspect or construct it in application code.

```ts
import {
  MakoLivePullStream,
  MakoReplicationSignals,
  createMakoPullOptions,
  createMakoPushOptions,
  type MakoCheckpoint,
} from "@mako-cloud/rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";

const live = new MakoLivePullStream<Todo>(config, auth);
const replication = replicateRxCollection<Todo, MakoCheckpoint>({
  replicationIdentifier: "mako-todos-v1",
  collection: database.todos,
  pull: createMakoPullOptions(config, auth, { stream$: live.stream$ }),
  push: createMakoPushOptions(config, auth),
  live: true,
});

const signals = new MakoReplicationSignals<Todo>();
const subscriptions = signals.bind(replication);
live.start();
await replication.awaitInitialReplication();
```

Close the SSE stream, unsubscribe the signals, and cancel replication when the owning application
scope is destroyed.

## Policy and local data

Mako evaluates document policy for every pull, push, and live event. Pull and live use the same
visibility rules. If a document was visible at an earlier checkpoint but is no longer visible, the
server sends a synthetic tombstone so RxDB removes the stale local copy. A denied push is a
non-retryable `permission_denied` error, and a conflict response includes a master document only
when the current user may read it.

Client-side filters are a user-interface convenience, not an authorization boundary. Applications
must also assume that previously synchronized data can remain in local storage until an
authorization-epoch reset completes. Do not render a protected collection while a security reset
or authentication-required state is active.

## Conflict handling

Mako follows RxDB's assumed-master-state protocol and returns readable master states as conflicts.
Set the RxDB collection's `conflictHandler` based on application semantics. This example chooses
the document with the larger application-managed version:

```ts
const conflictHandler = {
  isEqual(left: TodoState, right: TodoState) {
    return (
      left.id === right.id &&
      left.version === right.version &&
      left._deleted === right._deleted
    );
  },
  async resolve({ newDocumentState, realMasterState }: ConflictInput) {
    return newDocumentState.version > realMasterState.version
      ? newDocumentState
      : realMasterState;
  },
};
```

Use a server-issued logical version, hybrid logical clock, or another deterministic ordering value.
Do not rely on unsynchronized device wall clocks for business-critical conflict resolution. The
browser suite covers a concurrent offline edit where the newer remote state wins.

## Authorization-epoch security reset

Environment policy changes and user authorization changes increment epochs. Persist the epochs
with the replication generation and route every mismatch through
`MakoAuthorizationEpochCoordinator`:

```ts
import { MakoAuthorizationEpochCoordinator } from "@mako-cloud/rxdb";

const security = new MakoAuthorizationEpochCoordinator("mako-todos-v1", {
  async pauseReplication() {
    await replication.pause();
  },
  async clearReplicatedCollection() {
    await securelyRemoveLocalCollection();
  },
  onSecurityReset(event) {
    router.showAuthenticationBoundary(event.reason);
  },
  async startReplication(replicationIdentifier) {
    await createAndStartReplication(replicationIdentifier);
  },
});

await security.initialize({ environment: environmentEpoch, user: userEpoch });
await security.handleMismatch({ environment: nextEnvironmentEpoch, user: nextUserEpoch });
```

The required order is pause, securely clear, notify the application, and start with the newly
generated replication identifier. Do not resume an old replication metadata store after clearing
data. On access-token revocation, stop replication, clear protected local state, and require a new
sign-in; the reference browser test exercises this path.

## Schema migration and full resync

`schema_mismatch`, `checkpoint_expired`, stream gaps, and service failovers are explicit recovery
states, not ordinary retry loops:

```ts
import { MakoReplicationRecoveryCoordinator } from "@mako-cloud/rxdb";

const recovery = new MakoReplicationRecoveryCoordinator({
  async pauseReplication() {
    await replication.pause();
  },
  onSchemaMigrationRequired({ requiredSchemaVersion }) {
    migrationUi.open(requiredSchemaVersion);
  },
  onFullResyncRequired({ reason }) {
    resyncUi.confirmSecureReset(reason);
  },
});

replication.error$.subscribe((error) => void recovery.handleError(error));
const liveWithRecovery = new MakoLivePullStream(config, auth, {
  onResyncReason: (reason) => recovery.handleResyncReason(reason),
});
```

For a schema mismatch, stop using the collection, run the application's RxDB migration or install
the required schema, and create replication bound to that schema version. For a full resync,
securely clear the affected collection and its replication metadata, create a new replication
identifier, pull from the beginning, and call `markActive()` only after the application can safely
read the collection again.

## UI signals and retry behavior

`MakoReplicationSignals` exposes activity, received and sent documents, conflicts, throttling,
security resets, and sanitized errors. Its error values do not contain tokens or arbitrary response
bodies. Show `after_delay` throttles using their retry delay; do not retry errors whose advice is
`never`. Treat `authentication_required`, `schema_migration_required`, and
`full_resync_required` as blocking UI states rather than transient connectivity messages.

Run the tested reference application with:

```sh
npm run dev --workspace @mako-cloud/example-local-first
```

See the example README for browser-test setup and the exact covered scenarios.
