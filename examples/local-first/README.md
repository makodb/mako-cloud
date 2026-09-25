# Mako Cloud local-first reference app

This self-contained browser application demonstrates the supported RxDB integration. It uses the
real `@mako-cloud/rxdb` auth, pull, push, SSE, checkpoint, and signal adapters against a deterministic
in-browser implementation of the public Mako protocol.

The fake backend is test infrastructure, not a storage adapter or production server. It is one
implementation of the `ReferenceBackend` seam; `LiveMakoBackend` is the other, and points the same
application at a running deployment. An application supplies its own hosted endpoint the same way.

## Run it

From the repository root:

```sh
npm install
npm run build --workspace @mako-cloud/rxdb
npm run dev --workspace @mako-cloud/example-local-first
```

Open the URL printed by Vite. Use **Go offline**, add a todo, and use **Go online** to observe the
queued local write synchronize.

## Run the browser tests

Install Playwright's Chromium runtime once, then run the suite:

```sh
npx playwright install chromium
npm run test:browser --workspace @mako-cloud/example-local-first
```

The six scenarios verify:

- a write remains queryable offline and is pushed after reconnect;
- concurrent local and remote edits invoke the RxDB conflict handler;
- a remote tombstone removes the local document;
- an expiring access token is refreshed;
- a broken SSE stream reconnects and emits `RESYNC`;
- access revocation clears protected local state and requires authentication.

## Run the same scenarios against a real backend

The suite above proves the client half of the protocol; it would pass even if the server were
broken. To prove the server implements the other half, run the same six scenarios against real
service binaries:

```sh
cargo build --workspace --bins
npm run build --workspace @mako-cloud/rxdb
npm run test:browser-live --workspace @mako-cloud/example-local-first
```

That seeds a local tenant, starts a data plane, serves this application from an origin that also
proxies `/v1` to it, and drives the scenarios over real HTTP. Nothing is intercepted: the app signs
in for real, its writes are persisted, and every "remote" edit comes from a second authenticated
application user.

The live suite serves the page and the API from one origin, the way a deployment behind one reverse
proxy does. A page on another origin works too once that origin is in the environment's
[allowed origins](../../docs/user-book.md#allowed-origins-cors).

A live page that asks for credentials keeps the session in `localStorage` and the todos in IndexedDB,
under a database named for the project, environment, and user. A reload, or coming back later, stays
signed in, and a todo written offline is still there, and is pushed, after the tab was closed.
Signing out deletes both. The in-browser fake keeps everything in memory.

The implementation is in [`src/reference-app.ts`](src/reference-app.ts), the protocol fixture is in
[`src/mock-backend.ts`](src/mock-backend.ts), and the assertions are in
[`test/local-first.spec.ts`](test/local-first.spec.ts). Production setup, policy behavior, security
resets, and migration handling are documented in the [User Book](../../docs/user-book.md#building-a-local-first-app-with-rxdb).
