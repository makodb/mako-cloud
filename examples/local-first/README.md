# Mako Cloud local-first reference app

This self-contained browser application demonstrates the supported RxDB integration. It uses the
real `@mako-cloud/rxdb` auth, pull, push, SSE, checkpoint, and signal adapters against a deterministic
in-browser implementation of the public Mako protocol.

The fake backend is test infrastructure, not a storage adapter or production server. Replace its
injected `fetch` implementation with a hosted Mako endpoint in an application.

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

The implementation is in [`src/reference-app.ts`](src/reference-app.ts), the protocol fixture is in
[`src/mock-backend.ts`](src/mock-backend.ts), and the assertions are in
[`test/local-first.spec.ts`](test/local-first.spec.ts). Production setup, policy behavior, security
resets, and migration handling are documented in [`docs/rxdb-client.md`](../../docs/rxdb-client.md).
