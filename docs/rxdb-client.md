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

The package is published on its own and depends on nothing else from this repository; its npm
landing page is [`packages/rxdb-client/README.md`](../packages/rxdb-client/README.md).

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

Implement `AuthSessionPersistence` using the platform's protected credential storage, or use the
`BrowserAuthSessionPersistence` described below in a web application. The public
`MakoUserSession` intentionally omits the refresh token. `validAccessToken()` returns the stored
access token while it has more than 30 seconds of validity left and renews it otherwise.

### Session renewal

The refresh credential is single-use: the token route rotates it and treats a second spend outside
its narrow grace window as a replay, which revokes the whole refresh family. The client therefore
coalesces renewal. One refresh is in flight at a time; `refreshSession()` and `validAccessToken()`
callers -- across every replication scope, storage call, and live stream in the page -- await that
same request and resolve with the same session, so several scopes reacting to one
`authorization_epoch_changed` signal cannot spend the rotated credential twice. Once it settles,
the next call starts a new request.

A refresh either ends the session or does not touch it, and the client tells those apart by what
the service answered:

| Outcome | Stored session | Thrown | Flag and event |
| --- | --- | --- | --- |
| Rotated (`200`) | replaced | -- | `refreshUnavailable` cleared; `session`, plus `refresh_recovered` if it had been set |
| `401 unauthenticated` (invalid or replayed credential), `403 permission_denied`, any other `4xx` such as `400 invalid_request` | cleared | `MakoAuthenticationRequiredError` (`code: "unauthenticated"`, `retryable: false`) | `signed_out` |
| Network fault, timeout, `408`, `429` (`rate_limited` or `quota_exceeded`), any `5xx` | kept | `MakoAuthError` (`code: "refresh_unavailable"`, `retryable: true`) | `refreshUnavailable` set; `refresh_unavailable` on the transition |

Only a definitive refusal signs a user out. Under a transient failure the persisted session stays
exactly as it was, `authenticationRequired` stays `false`, and `refreshUnavailable` becomes `true`
until a later refresh succeeds. `validAccessToken()` then returns the current access token while it
has not yet expired, and throws the retryable `MakoAuthError` once it has -- it never throws
`MakoAuthenticationRequiredError` for a failure the service did not pronounce. A `429
quota_exceeded` is kept the same way even though its retry advice is `never`: the credential is
intact and the refresh succeeds once the quota window resets.

This is what an offline restart looks like. `restoreSession()` returns the stored session, the
expired access token cannot be renewed, and the application keeps reading and writing its local
RxDB data with `refreshUnavailable` set; replication resumes on its own when the network returns.
Pulls, pushes, and object requests report that state as a retryable `unavailable`
(`MakoReplicationError` / `MakoStorageError`) rather than `unauthenticated`, so RxDB retries
instead of the application tearing the session down. A live stream keeps reconnecting through it;
the stream ends only on a verdict -- `MakoAuthenticationRequiredError`, or a non-retryable
`MakoReplicationError` such as the `unauthenticated` one a refused stream request produces.

`subscribe(listener)` reports these transitions without polling and returns the function that stops
the subscription:

```ts
const stop = auth.subscribe((event) => {
  switch (event.kind) {
    case "session": // signed in, or a refresh rotated the session
    case "refresh_recovered":
      hideOfflineBanner();
      break;
    case "refresh_unavailable":
      showBanner("offline - working from local data");
      break;
    case "signed_out":
      requireSignIn();
      break;
  }
});
```

Each event carries `session`: the session in force when it was emitted, or `null` once signed out.
A listener that throws is ignored, so one subscriber cannot break another.

### Sign in through a provider

Provider sign-in ([auth-providers.md](auth-providers.md)) is a round trip through the browser: the
application asks where to send the user, the provider sends the browser back to a registered
redirect with a one-time code in the URL fragment, and the application exchanges the code for the
same session password sign-in issues. Only the public project key is needed on the way; no token
ever travels in a URL.

```ts
// On the sign-in screen: navigate to the provider.
const { authorizationUrl } = await auth.startProviderSignIn(
  "google",
  "https://app.example.com/auth/callback",
);
window.location.assign(authorizationUrl);

// On the redirect page: finish the exchange from the fragment the browser landed with.
const session = await auth.completeProviderSignIn(window.location.hash);
```

`startProviderSignIn(provider, redirectUrl)` returns `{ authorizationUrl, provider }`; the redirect
must be one the environment registered, matched exactly. `completeProviderSignIn(fragment)` accepts
`location.hash` with or without its leading `#`, exchanges `#code=…`, stores the session in the
configured persistence, and returns it. A provider refusal arrives as `#error=<reason>`; the helper
throws `MakoAuthError` with that reason in `error.reason` (for example `provider_refused` or
`email_not_verified`) without calling the service. Sessions obtained this way refresh, expose
`validAccessToken()`, and are revoked exactly like password sessions.

### Sign in with a magic link

```ts
await auth.requestMagicLink("person@example.com", "https://app.example.com/auth/magic");
// ...the mailed link lands on the redirect with `#magic_link_token=<token>`:
const session = await auth.redeemMagicLink(token);
```

`requestMagicLink` resolves when the service answers `202` and rejects with `MakoAuthError` on any
other status. It tells the application nothing about whether the address is registered, by design.
`redeemMagicLink(token)` trades the single-use token for a persisted session; a spent or expired
token is refused with `unauthenticated`.

### Handle the fragment on application load

Every redirect page can be reached by a provider callback, a magic link, or an ordinary
navigation. `MakoAuthClient.signInFragment(fragment)` classifies `location.hash` so the
application decides once, before rendering:

```ts
const fragment = MakoAuthClient.signInFragment(window.location.hash);
switch (fragment.kind) {
  case "provider_code":
    await auth.completeProviderSignIn(window.location.hash);
    break;
  case "magic_link":
    await auth.redeemMagicLink(fragment.value);
    break;
  case "error":
    showSignInError(fragment.value);
    break;
  case "none":
    await auth.restoreSession();
    break;
}
history.replaceState(null, "", window.location.pathname + window.location.search);
```

Clear the fragment after consuming it so a reload does not retry a spent code, and never log it.

### Keep the session across reloads in a browser

`BrowserAuthSessionPersistence` stores the session in `localStorage` under
`mako.auth.session.<projectId>.<environmentId>` (or a `key` you choose). When storage is
unavailable (a sandboxed frame, a blocked or full store), it degrades to memory for the lifetime
of the page and never throws; `durable` tells you which mode it is in. A malformed stored value is
discarded rather than trusted. `clear()` removes the stored session, and `signOut()` calls it.

```ts
import { BrowserAuthSessionPersistence, MakoAuthClient } from "@mako-cloud/rxdb";

const auth = new MakoAuthClient(config, {
  persistence: new BrowserAuthSessionPersistence(config),
});
```

`localStorage` is readable by any script on the origin, which is the trust boundary of a
single-page application; keep third-party scripts off the origin that holds sessions.

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

## Durable replication state with Dexie

RxDB's Dexie storage (`getRxStorageDexie` from `rxdb/plugins/storage-dexie`) keeps documents and
RxDB's own replication checkpoint in IndexedDB, so a reopened application shows its data before
any network request and pulls only what changed. The Mako-side state — the last checkpoint the
live stream reached, the authorization-epoch security state, and the recovery state — lives next
to it in `DexieReplicationStatePersistence`, an IndexedDB database of its own (default name
`mako-replication-state`, one key namespace per project, environment, and collection):

```ts
import {
  DexieReplicationStatePersistence,
  MakoAuthorizationEpochCoordinator,
  MakoLivePullStream,
  MakoReplicationRecoveryCoordinator,
  createMakoPullOptions,
  createMakoPushOptions,
} from "@mako-cloud/rxdb";
import { createRxDatabase } from "rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";
import { getRxStorageDexie } from "rxdb/plugins/storage-dexie";

const database = await createRxDatabase({ name: "rational", storage: getRxStorageDexie() });
const durable = new DexieReplicationStatePersistence(config);

const security = new MakoAuthorizationEpochCoordinator("mako-todos-v1", securityHooks, {
  persistence: durable.security,
});
const recovery = new MakoReplicationRecoveryCoordinator(recoveryHooks, {
  persistence: durable.recovery,
});

const securityState = await security.initialize({ environment: environmentEpoch, user: userEpoch });
const recoveryState = await recovery.initialize();
if (recoveryState.kind !== "active") {
  // Finish the migration or full resync recorded by the previous run first.
}

const live = new MakoLivePullStream<Todo>(config, auth, { checkpoints: durable.checkpoint });
const replication = replicateRxCollection<Todo, MakoCheckpoint>({
  replicationIdentifier: securityState.replicationIdentifier,
  collection: database.todos,
  pull: createMakoPullOptions(config, auth, {
    stream$: live.stream$,
    checkpoints: durable.checkpoint,
  }),
  push: createMakoPushOptions(config, auth),
  live: true,
});
live.start((await durable.checkpoint.load()) ?? undefined);
```

What each piece persists:

- `durable.checkpoint` — every checkpoint a pull returns or the live stream advances to, so
  `live.start(checkpoint)` resumes the SSE stream where it stopped. RxDB's own checkpoint in the
  Dexie storage stays authoritative for pulls; the persisted one only makes the resume precise.
- `durable.security` — the epochs, generation, and replication identifier. A restart under the same
  epochs reuses the persisted generation and identifier; a restart under different epochs clears
  the collection first (see the security reset below).
- `durable.recovery` — `schema_migration_required` and `full_resync_required` survive a restart, and
  `markActive()` records the return to normal.

A security reset clears the persisted checkpoint and recovery state through the coordinator, and
`durable.clear()` forgets everything for the collection; call it from a full-resync flow together
with removing the collection. The pieces are also usable on their own: any object implementing
`ReplicationStateStore` (`get`, `set`, `delete`) can back the persistence, and
`MemoryReplicationStateStore` is the deterministic choice for tests, which is how the package's own
unit tests prove that a restart resumes from the persisted checkpoint and that a reset clears it.
`DexieReplicationStateStore` fails at construction when no IndexedDB implementation exists rather
than silently keeping state in memory; pass `indexedDB` and `IDBKeyRange` to inject one.

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

## Replicating one slice of a collection

A user who belongs to several households may read the documents of all of them, so a database
per household receives every household's documents and discards what it did not want — paying for
the transfer, and re-examining every other household's changes on every pull. `filter` narrows the
scope to the documents whose field holds one value:

```ts
const config = normalizeMakoRxdbConfig({
  ...scope,
  collectionId: "transactions",
  filter: { field: "household_id", value: householdId },
});
```

It is applied **after** the policy, so it can only narrow what the caller was already allowed to
read; it is not an authorization boundary and the platform trusts nothing about it. The pull and
the live stream both use it — a stream wider than the pull delivers documents the database
discards, and one narrower withholds changes the pull would have sent, so the client sends the same
filter to both.

A document that leaves the filter comes back as a tombstone, exactly as one that leaves the
policy's reach does, so a transaction moved to another household disappears from the database it
left instead of sitting there for ever. The checkpoint and the stream cursor are bound to the
filter: resuming one under a different filter is refused rather than silently skipping every
change the other filter passed over. Changing an open database's filter therefore means a new
replication identifier and a fresh local store, the same as changing its collection.

## One stream for many collections

A browser opens six connections to one host. An application with a dozen collections therefore
cannot have a live stream each: the later streams queue behind the earlier ones, and the pulls and
pushes queue behind those, so the application looks connected and syncs nothing.
`MakoLiveStreamGroup` opens one connection for every collection it is given:

```ts
const group = createMakoLiveStreamGroup(configs, auth, { onResyncReason });
for (const config of configs) {
  await replicateRxCollection({
    collection: collections[config.collectionId],
    replicationIdentifier,
    pull: { ...createMakoPullOptions(config, auth), stream$: group.stream$(config.collectionId) },
    push: createMakoPushOptions(config, auth),
  });
}
```

Every collection on one connection must belong to one environment, because the connection is
authenticated and routed as that environment's — the group refuses a configuration that mixes
them. Each collection still keeps its own policy, checkpoint, cursor, and filter; sharing a
connection changes nothing about what a collection receives. Every event names the collection it
belongs to, and a reconnect sends each collection's own cursor back: one `Last-Event-ID` could
only speak for whichever event happened to be last, leaving every other collection resuming from a
position it never reached.

A collection whose events outrun the buffer resyncs on its own, and the connection stays up for
the rest. A frame the client cannot make sense of — unparseable, or naming no collection — is the
connection itself being untrustworthy, so every collection resyncs and it reconnects.

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
  async clearReplicatedCollection({ replicationRunning }) {
    // `replicationRunning: false` means nothing is open yet: clear by
    // database name, because there is no handle to clear through.
    await securelyRemoveLocalData({ byName: !replicationRunning });
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
generated replication identifier. Between clearing and starting, the coordinator also calls the
persistence's optional `clearReplicationState()` so a durable checkpoint and recovery state never
outlive the data they describe. Do not resume an old replication metadata store after clearing
data. On access-token revocation, stop replication, clear protected local state, and require a new
sign-in; the reference browser test exercises this path.

`initialize(epochs)` reuses the persisted state when the epochs match. When durable state was
persisted under different epochs — the user was removed from a group while the application was
closed, for example — it runs `clearReplicatedCollection`, clears the persisted replication state,
saves a new generation, and calls `onSecurityReset` before returning; nothing is running yet, so it
does not pause, and the caller starts replication with the returned identifier.

That case is why the clear is told whether replication was running. At startup there is no open
collection, so an implementation that clears through a handle it holds does nothing at all and the
previous generation's documents survive the restart — the one outcome a security reset exists to
prevent. With `replicationRunning: false`, remove the database by name.

Every transition runs alone. A live `authorization_epoch_changed` and the application's own epoch
sync after a write arrive together routinely, and a second `initialize` or `handleMismatch` waits
for the one in flight and then re-reads the settled state, so it returns that generation instead of
starting another reset. Overlapping resets clear a database the other is replicating into, which
RxDB reports as `DB8`.

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
read the collection again. With a `persistence` (`durable.recovery` above), the state survives a
restart: call `initialize()` before starting replication and act on a restored non-`active` state.

## Files in buckets

`MakoStorageClient` reads and writes bucket objects ([file-storage.md](file-storage.md)) under the
same public key and application-user session replication uses:

```ts
import { MakoStorageClient } from "@mako-cloud/rxdb";

const storage = new MakoStorageClient(config, auth);
const path = `households/${id}/transactions/${txn}/receipt.png`;
const { etag, size } = await storage.put("receipts", path, file, {
  contentType: file.type,
  ifNoneMatch: "*",
});
// { bytes, contentType, etag }, or null when the object does not exist.
const object = await storage.get("receipts", path);
const page = await storage.list("receipts", { prefix: `households/${id}/`, limit: 100 });
await storage.delete("receipts", path);
imageElement.src = storage.url("public-images", "logos/acme.png");
```

- `put` and `delete` require a session and refuse with `unauthenticated` before any request when
  none exists, or with a retryable `unavailable` when a session exists but its renewal cannot reach
  the service. `get` and `list` send the bearer when a session exists and go without it otherwise,
  so a public bucket answers anonymously and a policy bucket refuses.
- Bodies may be a `Blob`, `ArrayBuffer`, `Uint8Array`, or string; `contentType` is required and
  `ifNoneMatch` is forwarded verbatim as `If-None-Match`. `put` returns the quoted plaintext-digest
  ETag a later `get` answers with, and the stored size.
- Paths are percent-encoded segment by segment (`encodeObjectPath`), and a path that can never be
  valid — empty, `.` or `..` segments, control characters, more than 512 bytes — is refused locally.
- Failures are `MakoStorageError` with the API's `code`, `requestId`, `retry`, and `status`; a
  response without an error envelope maps to `unavailable` (5xx) or `internal` without echoing its
  body, and a network fault to a retryable `unavailable`.

`url()` builds the object's address for `<img src>` and links; it carries no credential, so it is
only useful on a public bucket.

## Replication errors and retry behavior

Every pull, push, and live-stream failure is a `MakoReplicationError` carrying the service's
`code`, `message`, `requestId`, `retry` advice and `status`, plus the two values a caller acts on:
`retryable`, and `retryAfterMilliseconds` for an `after_delay` advice. Two rules decide
`retryable`, in this order:

1. **The service's advice outranks the status.** `retry: {kind: "never"}` is terminal whatever the
   status was, a `500` included. `immediate` and `after_delay` are retryable, and an `after_delay`
   carries its delay onto the error so a caller can wait instead of repeating at once.
2. **A refused credential is terminal.** A `401`, or a `403` the service labelled
   `unauthenticated`, is classified `unauthenticated` and non-retryable even when its envelope
   advises a retry. Repeating a request with a credential the service has already rejected is a
   denial of service against your own backend, and the person sees a client that looks connected
   and never syncs.

| Failure | `code` | `retryable` |
| --- | --- | --- |
| `401`, or `403` with `unauthenticated`, with or without an envelope | `unauthenticated` | no |
| Any status whose envelope advises `never` -- `403 permission_denied`, `409 schema_mismatch`, `409 checkpoint_expired`, `500 internal` | the envelope's code | no |
| Any status whose envelope advises `immediate` or `after_delay` -- `429 rate_limited`, `503 unavailable` | the envelope's code | yes |
| `5xx` with no envelope (a proxy or gateway answered) | `unavailable` | yes, `immediate` |
| `408` or `429` with no envelope | `internal` | yes, `after_delay` 1 s |
| Any other `4xx` with no envelope | `internal` | no |
| Network fault, or a response body that is not the documented shape | `unavailable` | yes, `immediate` |
| A renewal that never reached a verdict: offline, `408`, `429`, `5xx` | `unavailable` | yes, `after_delay` 1 s |
| No session at all, or a renewal the service definitively refused | `unauthenticated` | no |

A `denied` push row keeps the policy error the service returned for it, ordinarily a non-retryable
`permission_denied`.

### One renewal, never a loop

A `401` is ordinarily nothing worse than an expired access token, so a pull, a push, and a stream
connection each renew the session once and repeat the request exactly once with the renewed
bearer. The renewal is the coalesced `refreshSession()`, so collections failing together share a
single token request. A second refusal is definitive: the client remembers the refused token,
raises the terminal `unauthenticated` error, and every later attempt carrying that same token
fails on one request without renewing again -- a repeat costs one refused request, never a growing
storm. A refused push is repeated byte for byte under its original `Idempotency-Key`, so a renewal
cannot duplicate a write.

A renewal that could not reach a verdict is not a refusal: the session is kept and the retryable
`unavailable` above is raised instead, exactly as on an offline restart.

### Stopping on a terminal error

RxDB's replication protocol has no notion of a non-retryable failure: it repeats a failed handler
on its own `retryTime` for as long as replication runs. **An application must therefore stop
replication itself when a terminal error arrives.** The live stream is owned by this package and
does stop on its own -- it errors `stream$` and closes rather than reconnecting against a verdict.

```ts
signals.activity$.subscribe((activity) => {
  if (activity === "authentication_required") {
    void replication.cancel();
    live.close();
    requireSignIn();
  }
});
```

`unauthenticated` means the session is gone, not that it is busy: cancel replication, close the
live stream, and send the person through sign-in again. Do not renew or retry it in application
code -- the client already made the one renewal attempt that was worth making. After a new
sign-in, create replication again (through `MakoAuthorizationEpochCoordinator` if the epochs
moved) and start a new stream. Local data stays readable throughout; treat it as described in
[Policy and local data](#policy-and-local-data).

Errors reach `error$` wrapped in RxDB's own `RC_PULL` / `RC_PUSH` error, which keeps only part of
the original. `makoReplicationErrorFrom(error)` returns the `MakoReplicationError` inside one, or
`null` for a failure this client did not raise; `MakoReplicationSignals` and
`MakoReplicationRecoveryCoordinator.handleError` already unwrap it, so subscribe to them, or call
it yourself when subscribing to `error$` directly.

### UI signals

`MakoReplicationSignals` exposes activity, received and sent documents, conflicts, throttling,
security resets, and sanitized errors. Its error values do not contain tokens or arbitrary response
bodies. Show `after_delay` throttles using their retry delay; do not retry errors whose advice is
`never`. Treat `authentication_required`, `schema_migration_required`, and
`full_resync_required` as blocking UI states rather than transient connectivity messages. A
retryable `unavailable` from a renewal that could not reach the service is a connectivity message;
pair it with `MakoAuthClient.refreshUnavailable` and its `refresh_unavailable` /
`refresh_recovered` events.

Run the tested reference application with:

```sh
npm run dev --workspace @mako-cloud/example-local-first
```

See the example README for browser-test setup and the exact covered scenarios.
