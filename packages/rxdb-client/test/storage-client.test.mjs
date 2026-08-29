import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoStorageClient,
  MakoStorageError,
  encodeObjectPath,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

const objectsUrl =
  "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/storage/receipts/objects";

function record(overrides = {}) {
  return {
    bucketId: "receipts",
    path: "households/h1/transactions/t1/receipt.png",
    contentType: "image/png",
    sizeBytes: 3,
    ownerId: "usr_abcdefgh",
    digest: "sha256:abc",
    storedDigest: "sha256:def",
    createdAtUnixSeconds: 1,
    updatedAtUnixSeconds: 2,
    ...overrides,
  };
}

function apiError(code, status, requestId = "req_storage") {
  return Response.json(
    {
      apiVersion: "v1",
      error: { code, message: `${code} happened`, requestId, retry: { kind: "never" } },
    },
    { status },
  );
}

async function signedIn(fetch) {
  const auth = new MakoAuthClient(config(), { fetch, now: () => 0 });
  await auth.signInWithPassword("user@example.test", "password");
  return auth;
}

function fakeFetch(handler) {
  const requests = [];
  const fetch = async (input, init = {}) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json({
        accessToken: "access-token",
        refreshToken: "refresh-token",
        expiresIn: 600,
        user: { id: "usr_abcdefgh", email: "u@example.test", status: "active", authorizationEpoch: 1 },
      });
    }
    requests.push({ url: String(input), init });
    return handler(String(input), init);
  };
  return { fetch, requests };
}

test("puts an object with the key, bearer, content type, and conditional headers", async () => {
  const { fetch, requests } = fakeFetch(() => Response.json(record()));
  const storage = new MakoStorageClient(config(), await signedIn(fetch), { fetch });
  const bytes = new Uint8Array([1, 2, 3]);
  const result = await storage.put("receipts", "households/h1/transactions/t1/receipt.png", bytes, {
    contentType: "image/png",
    ifNoneMatch: "*",
  });
  assert.deepEqual(result, { etag: '"sha256:abc"', size: 3 });
  assert.equal(requests[0].url, `${objectsUrl}/households/h1/transactions/t1/receipt.png`);
  assert.equal(requests[0].init.method, "PUT");
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(requests[0].init.headers.Authorization, "Bearer access-token");
  assert.equal(requests[0].init.headers["Content-Type"], "image/png");
  assert.equal(requests[0].init.headers["If-None-Match"], "*");
  assert.deepEqual(Array.from(requests[0].init.body), [1, 2, 3]);

  await storage.put("receipts", "notes/a b.txt", "hello", { contentType: "text/plain" });
  assert.equal(requests[1].url, `${objectsUrl}/notes/a%20b.txt`);
  assert.equal(requests[1].init.headers["If-None-Match"], undefined);
  assert.equal(requests[1].init.body, "hello");
  await storage.put("receipts", "blob.bin", new Blob([bytes]), {
    contentType: "application/octet-stream",
  });
  assert.equal(requests[2].init.body instanceof Blob, true);
  await storage.put("receipts", "buffer.bin", bytes.buffer, {
    contentType: "application/octet-stream",
  });
  assert.equal(requests[3].init.body instanceof ArrayBuffer, true);
});

test("percent-encodes path segments and refuses paths that can never be valid", () => {
  assert.equal(encodeObjectPath("a/b c/d#e?f.png"), "a/b%20c/d%23e%3Ff.png");
  assert.equal(encodeObjectPath("ünïcode/ok"), "%C3%BCn%C3%AFcode/ok");
  for (const path of ["", "a//b", "/a", "a/", "../a", "a/./b", "a\tb", "x".repeat(513)]) {
    assert.throws(
      () => encodeObjectPath(path),
      (error) => error instanceof MakoStorageError && error.code === "invalid_request",
    );
  }
});

test("gets an object with its content type and etag, and answers null for a missing one", async () => {
  const { fetch, requests } = fakeFetch((url) => {
    if (url.endsWith("/missing.png")) {
      return apiError("not_found", 404);
    }
    return new Response(new Uint8Array([9, 8, 7]), {
      status: 200,
      headers: { "content-type": "image/png", etag: '"sha256:abc"' },
    });
  });
  const storage = new MakoStorageClient(config(), await signedIn(fetch), { fetch });
  const object = await storage.get("receipts", "households/h1/receipt.png");
  assert.deepEqual(Array.from(object.bytes), [9, 8, 7]);
  assert.equal(object.contentType, "image/png");
  assert.equal(object.etag, '"sha256:abc"');
  assert.equal(requests[0].init.method, "GET");
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(requests[0].init.headers.Authorization, "Bearer access-token");
  assert.equal(await storage.get("receipts", "missing.png"), null);
});

test("reads without a bearer when no session exists, so public buckets work anonymously", async () => {
  const { fetch, requests } = fakeFetch(() => Response.json({ items: [], nextCursor: null }));
  const auth = new MakoAuthClient(config(), { fetch });
  const storage = new MakoStorageClient(config(), auth, { fetch });
  assert.deepEqual(await storage.list("public-images"), { items: [], nextCursor: null });
  assert.equal(requests[0].init.headers.Authorization, undefined);
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(
    storage.url("public-images", "logos/acme logo.png"),
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/storage/public-images/objects/logos/acme%20logo.png",
  );
  await assert.rejects(
    () => storage.put("public-images", "x.png", "x", { contentType: "image/png" }),
    (error) => error instanceof MakoStorageError && error.code === "unauthenticated",
  );
  await assert.rejects(
    () => storage.delete("public-images", "x.png"),
    (error) => error instanceof MakoStorageError && error.code === "unauthenticated",
  );
  assert.equal(requests.length, 1);
});

test("lists with prefix, limit, and cursor and follows the next cursor", async () => {
  const { fetch, requests } = fakeFetch((url) =>
    Response.json({
      items: [record({ path: "households/h1/a.png" })],
      nextCursor: url.includes("cursor=") ? null : "households/h1/a.png",
    }),
  );
  const storage = new MakoStorageClient(config(), await signedIn(fetch), { fetch });
  const first = await storage.list("receipts", { prefix: "households/h1/", limit: 1 });
  assert.equal(first.items[0].path, "households/h1/a.png");
  assert.equal(first.nextCursor, "households/h1/a.png");
  const url = new URL(requests[0].url);
  assert.equal(url.pathname.endsWith("/storage/receipts/objects"), true);
  assert.equal(url.searchParams.get("prefix"), "households/h1/");
  assert.equal(url.searchParams.get("limit"), "1");
  assert.equal(requests[0].init.headers.Authorization, "Bearer access-token");
  const second = await storage.list("receipts", { cursor: first.nextCursor });
  assert.equal(second.nextCursor, null);
  assert.equal(new URL(requests[1].url).searchParams.get("cursor"), "households/h1/a.png");
  await assert.rejects(
    () => storage.list("receipts", { limit: 0 }),
    (error) => error instanceof MakoStorageError && error.code === "invalid_request",
  );
});

test("deletes with the session and resolves without a body", async () => {
  const { fetch, requests } = fakeFetch(() => new Response(null, { status: 204 }));
  const storage = new MakoStorageClient(config(), await signedIn(fetch), { fetch });
  assert.equal(await storage.delete("receipts", "households/h1/a.png"), undefined);
  assert.equal(requests[0].init.method, "DELETE");
  assert.equal(requests[0].url, `${objectsUrl}/households/h1/a.png`);
  assert.equal(requests[0].init.headers.Authorization, "Bearer access-token");
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
});

test("maps API errors, opaque failures, and network faults onto MakoStorageError", async () => {
  const { fetch } = fakeFetch((url) => {
    if (url.endsWith("/denied.png")) {
      return apiError("permission_denied", 403, "req_denied");
    }
    if (url.endsWith("/opaque.png")) {
      return new Response("<html>bad gateway</html>", {
        status: 502,
        headers: { "x-request-id": "req_gateway" },
      });
    }
    if (url.endsWith("/malformed.png")) {
      return Response.json({ unexpected: true });
    }
    throw new TypeError("fetch failed");
  });
  const storage = new MakoStorageClient(config(), await signedIn(fetch), { fetch });
  await assert.rejects(
    () => storage.get("receipts", "denied.png"),
    (error) =>
      error instanceof MakoStorageError &&
      error.code === "permission_denied" &&
      error.requestId === "req_denied" &&
      error.status === 403 &&
      error.retryable === false,
  );
  await assert.rejects(
    () => storage.get("receipts", "opaque.png"),
    (error) =>
      error instanceof MakoStorageError &&
      error.code === "unavailable" &&
      error.requestId === "req_gateway" &&
      error.status === 502 &&
      error.retryable === true &&
      !error.message.includes("html"),
  );
  await assert.rejects(
    () => storage.put("receipts", "malformed.png", "x", { contentType: "image/png" }),
    (error) => error instanceof MakoStorageError && error.code === "internal",
  );
  await assert.rejects(
    () => storage.get("receipts", "offline.png"),
    (error) =>
      error instanceof MakoStorageError && error.code === "unavailable" && error.retryable === true,
  );
});

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "browser",
  });
}
