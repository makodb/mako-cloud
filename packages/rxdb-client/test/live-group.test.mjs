import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  createMakoLiveStreamGroup,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

// A browser opens six connections to one host, so an application with a dozen
// collections cannot have a stream each. One connection carries them, and
// every event names the collection it is for.
test("one connection routes each collection's events to its own stream", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    requests.push({ url: String(input), body: JSON.parse(String(init.body)) });
    return new Response(
      sse({
        event: "documents",
        collection: "transactions",
        data: {
          documents: [{ id: "txn-1", _deleted: false }],
          checkpoint: "mcp1.txn",
          cursor: "msc1.txn",
        },
      }) +
        sse({
          event: "documents",
          collection: "accounts",
          data: {
            documents: [{ id: "acc-1", _deleted: false }],
            checkpoint: "mcp1.acc",
            cursor: "msc1.acc",
          },
        }),
      { headers: { "content-type": "text/event-stream" } },
    );
  };
  const auth = new MakoAuthClient(config("transactions"), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const group = createMakoLiveStreamGroup([config("transactions"), config("accounts")], auth, {
    fetch,
    reconnectMinimumDelayMs: 10,
    reconnectMaximumDelayMs: 10,
  });

  const seen = { transactions: [], accounts: [] };
  const settled = new Promise((resolve) => {
    let remaining = 2;
    for (const name of ["transactions", "accounts"]) {
      group.stream$(name).subscribe((event) => {
        seen[name].push(event);
        remaining -= 1;
        if (remaining === 0) {
          group.close();
          resolve();
        }
      });
    }
  });
  await settled;

  assert.equal(seen.transactions.length, 1);
  assert.equal(seen.accounts.length, 1);
  assert.equal(seen.transactions[0].documents[0].id, "txn-1");
  assert.equal(seen.accounts[0].documents[0].id, "acc-1");
  // Each collection keeps its own checkpoint; nothing is shared but the wire.
  assert.deepEqual(group.checkpointOf("transactions"), { token: "mcp1.txn" });
  assert.deepEqual(group.checkpointOf("accounts"), { token: "mcp1.acc" });

  // One request, naming both collections, to the environment's stream.
  assert.equal(requests.length, 1);
  assert.ok(requests[0].url.endsWith("/environments/env_abcdefgh/replication/stream"));
  assert.deepEqual(
    requests[0].body.collections.map((entry) => entry.collectionId),
    ["transactions", "accounts"],
  );
});

test("a reconnect sends every collection's own cursor", async () => {
  const requests = [];
  let opened = 0;
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    requests.push(JSON.parse(String(init.body)));
    opened += 1;
    if (opened === 1) {
      // One collection advances, then the connection ends.
      return new Response(
        sse({
          event: "checkpoint",
          collection: "transactions",
          data: { checkpoint: "mcp1.txn", cursor: "msc1.txn" },
        }),
        { headers: { "content-type": "text/event-stream" } },
      );
    }
    return new Response(
      sse({
        event: "checkpoint",
        collection: "accounts",
        data: { checkpoint: "mcp1.acc", cursor: "msc1.acc" },
      }),
      { headers: { "content-type": "text/event-stream" } },
    );
  };
  const auth = new MakoAuthClient(config("transactions"), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const group = createMakoLiveStreamGroup([config("transactions"), config("accounts")], auth, {
    fetch,
    reconnectMinimumDelayMs: 1,
    reconnectMaximumDelayMs: 1,
  });
  const settled = new Promise((resolve) => {
    group.stream$("accounts").subscribe(() => {
      group.close();
      resolve();
    });
  });
  group.stream$("transactions").subscribe(() => {});
  await settled;

  assert.ok(requests.length >= 2, "the stream reconnected");
  const reconnect = requests[requests.length - 1];
  const byId = Object.fromEntries(
    reconnect.collections.map((entry) => [entry.collectionId, entry]),
  );
  // The collection that advanced resumes from its own cursor; the one that
  // never received anything asks for nothing rather than borrowing the
  // other's position.
  assert.equal(byId.transactions.cursor, "msc1.txn");
  assert.equal(byId.accounts.cursor, undefined);
});

test("collections on one stream must share an environment", () => {
  const other = normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_otherenv",
    collectionId: "accounts",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  });
  const auth = new MakoAuthClient(config("transactions"), { fetch: async () => new Response("") });
  assert.throws(
    () => createMakoLiveStreamGroup([config("transactions"), other], auth),
    /share its environment/u,
  );
  assert.throws(
    () => createMakoLiveStreamGroup([config("transactions"), config("transactions")], auth),
    /named twice/u,
  );
  assert.throws(() => createMakoLiveStreamGroup([], auth), /at least one collection/u);
});

function sse(event) {
  return `event: ${event.event}\ndata: ${JSON.stringify(event)}\n\n`;
}

function session() {
  return {
    accessToken: "access-token",
    refreshToken: "refresh-token",
    expiresIn: 900,
    user: {
      id: "usr_abcdefgh",
      email: "user@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  };
}

function config(collectionId) {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId,
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  });
}
