import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoReplicationError,
  createMakoLivePullStream,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

test("parses SSE documents and checkpoint events for RxDB", async () => {
  const calls = [];
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    calls.push({ url: String(input), headers: init.headers });
    return new Response(
      sse({
        event: "documents",
        data: {
          documents: [{ id: "todo-1", _deleted: false }],
          checkpoint: "mcp1.next",
          cursor: "msc1.next",
        },
      }) +
        sse({
          event: "checkpoint",
          data: { checkpoint: "mcp1.hidden", cursor: "msc1.hidden" },
        }),
      { headers: { "content-type": "text/event-stream" } },
    );
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const stream = createMakoLivePullStream(config(), auth, {
    fetch,
    reconnectMinimumDelayMs: 10,
    reconnectMaximumDelayMs: 10,
  });
  const events = [];
  const complete = new Promise((resolve) => {
    stream.stream$.subscribe((event) => {
      events.push(event);
      if (events.length === 2) {
        stream.close();
        resolve();
      }
    });
  });
  stream.start({ token: "mcp1.initial" });
  await complete;
  assert.equal(events[0].documents[0].id, "todo-1");
  assert.deepEqual(events[1], { documents: [], checkpoint: { token: "mcp1.hidden" } });
  assert.equal(new URL(calls[0].url).searchParams.get("checkpoint"), "mcp1.initial");
  assert.equal(calls[0].headers.Authorization, "Bearer access-token");
});

test("turns bounded-buffer overflow into RESYNC", async () => {
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    return new Response(
      sse({
        event: "checkpoint",
        data: { checkpoint: "mcp1.one", cursor: "msc1.one" },
      }) +
        sse({
          event: "checkpoint",
          data: { checkpoint: "mcp1.two", cursor: "msc1.two" },
        }),
    );
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const stream = createMakoLivePullStream(config(), auth, {
    fetch,
    maximumBufferedEvents: 1,
    reconnectMinimumDelayMs: 10,
    reconnectMaximumDelayMs: 10,
  });
  const event = await new Promise((resolve) => {
    stream.stream$.subscribe((value) => {
      stream.close();
      resolve(value);
    });
    stream.start();
  });
  assert.equal(event, "RESYNC");
});

test("renews once on a refused stream and ends it rather than reconnecting forever", async () => {
  const counts = { stream: 0, token: 0 };
  const bearers = [];
  const fetch = async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/signin")) {
      return Response.json(session());
    }
    if (url.endsWith("/auth/token")) {
      counts.token += 1;
      return Response.json({ ...session(), accessToken: "renewed-access-token" });
    }
    counts.stream += 1;
    bearers.push(init.headers.Authorization);
    return Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "unauthenticated",
          message: "the access token is not valid",
          requestId: "req_401",
          retry: { kind: "never" },
        },
      },
      { status: 401 },
    );
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const stream = createMakoLivePullStream(config(), auth, {
    fetch,
    reconnectMinimumDelayMs: 10,
    reconnectMaximumDelayMs: 10,
  });
  const events = [];
  const failure = await new Promise((resolve) => {
    stream.stream$.subscribe({
      next: (event) => events.push(event),
      error: resolve,
    });
    stream.start();
  });
  assert.ok(failure instanceof MakoReplicationError);
  assert.equal(failure.code, "unauthenticated");
  assert.equal(failure.retryable, false);
  assert.deepEqual(counts, { stream: 2, token: 1 });
  assert.deepEqual(bearers, ["Bearer access-token", "Bearer renewed-access-token"]);
  assert.deepEqual(events, []);
  // The stream is finished: nothing reconnects behind the error.
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.deepEqual(counts, { stream: 2, token: 1 });
});

function sse(value) {
  return `event: ${value.event}\ndata: ${JSON.stringify(value)}\n\n`;
}

function session() {
  return {
    accessToken: "access-token",
    refreshToken: "refresh-token-value-that-is-long-enough",
    expiresIn: 300,
    user: {
      id: "usr_abcdefgh",
      email: "user@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  };
}

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  });
}
