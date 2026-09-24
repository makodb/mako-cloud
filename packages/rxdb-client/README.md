# @mako-cloud/rxdb

The supported client for building an application on [Mako Cloud](https://github.com/makodb/mako-cloud).

One package covers the three things an application needs from the platform:

- **Authentication** — application-user sign-up and sign-in by password, external provider, or
  magic link, with session storage, coalesced renewal, and events an offline banner can subscribe
  to.
- **Replication** — adapters that connect an ordinary [RxDB](https://rxdb.info) collection to
  Mako's authenticated pull, push, and server-sent-event endpoints, plus durable checkpoints,
  authorization-epoch resets, and schema-migration and full-resync recovery.
- **Storage** — reading and writing bucket objects under the same session.

It is local-first by construction: reads come from the device, writes queue while offline, and
replication catches up when the network returns.

This package speaks only the application surface. It carries a public project key
(`mako_pk.…`, safe to ship in a browser or mobile bundle) and an application-user session — never a
developer session, an automation token, or a service credential — and it never reaches the
management or operator API.

## Install

The package is not yet published to npm. Install the built `v0.2.0` release from the
public [distribution repository](https://github.com/makodb/mako-rxdb) over HTTPS:

```sh
npm install https://codeload.github.com/makodb/mako-rxdb/tar.gz/refs/tags/v0.2.0 rxdb@17 rxjs@7
```

The archive includes the compiled package, so no monorepo checkout or build is needed.
Keep importing from `@mako-cloud/rxdb`; only the installation source differs. Commit your
package lockfile to preserve the resolved archive and integrity hash.

`rxdb` and `rxjs` are peer dependencies, so your application controls their versions:

| Peer   | Supported range     |
| ------ | ------------------- |
| `rxdb` | `>=17.0.0 <18.0.0`  |
| `rxjs` | `>=7.8.0 <8.0.0`    |

Node 20.19 or newer, or any modern browser. The package is ESM only and ships browser and Node
builds behind one entry point; an unsupported RxDB major is refused before replication starts.

## Quick start

Sign in, replicate a collection into IndexedDB, and read and write an object — the whole loop.

```ts
import {
  BrowserAuthSessionPersistence,
  DexieReplicationStatePersistence,
  MakoAuthClient,
  MakoLivePullStream,
  MakoStorageClient,
  createMakoPullOptions,
  createMakoPushOptions,
  normalizeMakoRxdbConfig,
  type MakoCheckpoint,
} from "@mako-cloud/rxdb";
import { createRxDatabase } from "rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";
import { getRxStorageDexie } from "rxdb/plugins/storage-dexie";
import { RXDB_VERSION } from "rxdb/plugins/utils";

// 1. One normalized configuration per replicated collection.
const config = normalizeMakoRxdbConfig({
  endpoint: "https://api.example.mako.cloud",
  projectId: "prj_abcdefgh",
  environmentId: "env_abcdefgh",
  collectionId: "todos",
  schemaVersion: 1,
  publicProjectKey: "mako_pk.example",
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
});

// 2. An application-user session, kept across reloads.
const auth = new MakoAuthClient(config, {
  persistence: new BrowserAuthSessionPersistence(config),
});
await auth.restoreSession();
if (auth.currentSession() === null) {
  await auth.signInWithPassword("person@example.com", password);
}

// 3. A local database, then replication on top of it.
const database = await createRxDatabase({ name: "todos", storage: getRxStorageDexie() });
await database.addCollections({ todos: { schema: todoSchema } });

const durable = new DexieReplicationStatePersistence(config);
const live = new MakoLivePullStream<Todo>(config, auth, { checkpoints: durable.checkpoint });
const replication = replicateRxCollection<Todo, MakoCheckpoint>({
  replicationIdentifier: "mako-todos-v1",
  collection: database.todos,
  pull: createMakoPullOptions(config, auth, {
    stream$: live.stream$,
    checkpoints: durable.checkpoint,
  }),
  push: createMakoPushOptions(config, auth),
  live: true,
});
live.start((await durable.checkpoint.load()) ?? undefined);
await replication.awaitInitialReplication();

// Reads and writes are local; replication carries them both ways.
await database.todos.insert({ id: "t1", title: "Buy milk", done: false });
const open = await database.todos.find({ selector: { done: false } }).exec();

// 4. Files live in buckets, under the same session.
const storage = new MakoStorageClient(config, auth);
const { etag } = await storage.put("receipts", "2026/receipt.png", file, {
  contentType: file.type,
});
const receipt = await storage.get("receipts", "2026/receipt.png"); // or null
```

Close the live stream and cancel replication when the owning application scope is destroyed.

## Durable local storage

RxDB's Dexie storage (`getRxStorageDexie`) keeps documents and RxDB's own replication checkpoint in
IndexedDB, so a reopened application renders its data before making a single request and then pulls
only what changed.

`DexieReplicationStatePersistence` keeps the Mako-side state next to it, in an IndexedDB database of
its own (`mako-replication-state` by default, one key namespace per project, environment, and
collection):

- `durable.checkpoint` — every checkpoint a pull returns or the live stream advances to, so
  `live.start(checkpoint)` resumes the stream where it stopped.
- `durable.security` — the authorization epochs, generation, and replication identifier, so a
  restart under changed epochs clears the collection before replicating again.
- `durable.recovery` — a pending schema migration or full resync survives a restart, and
  `markActive()` records the return to normal.

Any object implementing `ReplicationStateStore` (`get`, `set`, `delete`) can back the persistence;
`MemoryReplicationStateStore` is the deterministic choice for tests. `DexieReplicationStateStore`
fails at construction when no IndexedDB implementation exists rather than silently keeping state in
memory — pass `indexedDB` and `IDBKeyRange` to inject one.

Browser sessions use `BrowserAuthSessionPersistence`, which stores the session in `localStorage` and
degrades to memory (never throwing) when storage is unavailable; `durable` tells you which mode it
is in. `localStorage` is readable by any script on the origin, so keep third-party scripts off an
origin that holds sessions.

## Session renewal, and what happens offline

The refresh credential is single-use, so the client coalesces renewal: one refresh is in flight at a
time and every caller — `refreshSession()`, `validAccessToken()`, each replication scope, each
storage call — awaits that same request and resolves with the same session.

A refresh either ends the session or leaves it untouched, and the two are never confused:

| Outcome                                                                | Stored session | Result                                                                 |
| ---------------------------------------------------------------------- | -------------- | ---------------------------------------------------------------------- |
| Rotated (`200`)                                                        | replaced       | `session` event, plus `refresh_recovered` if an outage had been flagged |
| `401`, `403`, or any other `4xx` — the service refused the credential   | cleared        | `MakoAuthenticationRequiredError`, `signed_out` event                   |
| Network fault, timeout, `408`, `429`, any `5xx` — no verdict at all     | kept           | retryable `MakoAuthError`, `refreshUnavailable` set                     |

Only a definitive refusal signs a user out. Under a transient failure the application keeps reading
and writing its local data with `refreshUnavailable` set; pulls, pushes, and object requests report a
retryable `unavailable` rather than `unauthenticated`, so RxDB retries instead of the application
tearing the session down. Subscribe to the transitions without polling:

```ts
const stop = auth.subscribe((event) => {
  switch (event.kind) {
    case "session":
    case "refresh_recovered":
      hideOfflineBanner();
      break;
    case "refresh_unavailable":
      showBanner("offline — working from local data");
      break;
    case "signed_out":
      requireSignIn();
      break;
  }
});
```

A session the service refuses outright is a different story. A `401` on a pull, push, or stream is
renewed once — the ordinary cause is an expired access token — and, when the renewed credential is
refused too, raised as a non-retryable `unauthenticated` `MakoReplicationError`; the client never
repeats a credential the service has already rejected. RxDB itself retries every handler failure on
its own schedule and has no notion of a terminal one, so cancel replication, close the live stream,
and take the person to sign-in when that error arrives. The live stream, owned by this package,
ends itself.

## Documentation

The full guide — provider and magic-link sign-in, redirect-fragment handling, conflict handling,
authorization-epoch resets, schema migration and full resync, bucket objects, the replication error
and retry rules, and the UI signals — is in the [Mako Cloud User Book](https://github.com/makodb/mako-cloud/blob/main/docs/user-book.md#building-a-local-first-app-with-rxdb).

## License

Apache-2.0. See [LICENSE](./LICENSE).
