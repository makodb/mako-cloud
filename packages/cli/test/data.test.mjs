// Explorer commands (grant issued, capability sent, grant revoked, capability
// never printed) and data jobs (export download, import upload with digest,
// dry run, confirmation) against the loopback mock, which also serves the
// artifact grant URLs so one server handles the ndjson PUT and GET.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  configDir,
  ENVIRONMENT_ID,
  PROJECT_ID,
  runCli,
  signedIn,
  startMockApi,
} from "./harness.mjs";

const COLLECTION_ID = "col_abcdefgh";
const JOB_ID = "job_abcdefgh";
const GRANT_ID = `xgr_${"a".repeat(32)}`;
const CAPABILITY = "mx1_capability-secret-never-printed-0123456789abcdef";
const ARTIFACT_GRANT = "g".repeat(96);
const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const GRANTS = `${BASE}/explorer/grants`;
const EXPLORER = `${BASE}/explorer/collections/${COLLECTION_ID}`;
const JOBS = `${BASE}/data-jobs`;
const JOB = `${JOBS}/${JOB_ID}`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const WAIT_FAST = { MAKO_WAIT_INTERVAL_MS: "1" };

function sha256(text) {
  return createHash("sha256").update(text).digest("hex");
}

function explorerGrant(overrides = {}) {
  return {
    grantId: GRANT_ID,
    capability: CAPABILITY,
    mode: "administrative",
    operations: ["get"],
    applicationUserId: null,
    issuedAtUnixSeconds: 1_700_000_000,
    expiresAtUnixSeconds: 1_700_000_060,
    authorizationEpoch: 1,
    ...overrides,
  };
}

function explorerDocument(overrides = {}) {
  return {
    documentId: "doc_1",
    revision: "1-abc",
    schemaVersion: 3,
    deleted: false,
    content: { id: "doc_1", title: "hello" },
    ...overrides,
  };
}

function dataJob(overrides = {}) {
  return {
    jobId: JOB_ID,
    kind: "export",
    state: "queued",
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: COLLECTION_ID,
    creatorId: "dev_abcdefgh",
    conflictStrategy: null,
    progress: { processed: 0, committed: 0, failed: 0, skipped: 0, exported: 0, bytes: 0 },
    errors: [],
    manifest: null,
    createdAtUnixSeconds: 1_700_000_000,
    updatedAtUnixSeconds: 1_700_000_000,
    expiresAtUnixSeconds: 1_700_086_400,
    ...overrides,
  };
}

function manifest(overrides = {}) {
  return {
    formatVersion: 1,
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: COLLECTION_ID,
    schemaVersion: 3,
    snapshot: null,
    rowCount: 2,
    byteCount: 40,
    digest: `sha256:${"m".repeat(64)}`,
    finalizedAtUnixSeconds: 1_700_000_010,
    ...overrides,
  };
}

/** Issues and revokes explorer grants; everything else goes to `explorer`. */
function explorerHandler(explorer) {
  return (request) => {
    if (request.method === "POST" && request.path === GRANTS) {
      return {
        status: 201,
        json: explorerGrant({ mode: request.body.mode, operations: request.body.operations }),
      };
    }
    if (request.method === "DELETE" && request.path === `${GRANTS}/${GRANT_ID}`) {
      return { status: 200, json: { grantId: GRANT_ID, revokedAtUnixSeconds: 1_700_000_001 } };
    }
    if (request.path.startsWith(EXPLORER) && request.headers["x-mako-explorer-capability"] !== CAPABILITY) {
      return apiError("permission_denied", "explorer capability is missing", 403);
    }
    return explorer(request);
  };
}

function neverPrinted(result, secret) {
  assert.equal(result.stdout.includes(secret), false, "secret in stdout");
  assert.equal(result.stderr.includes(secret), false, "secret in stderr");
}

test("explorer get issues a grant, reads with the capability, revokes it, and never prints it", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: explorerHandler((request) => {
        if (request.method === "GET" && request.path === `${EXPLORER}/documents/doc_1`) {
          return { status: 200, json: explorerDocument() };
        }
        return undefined;
      }),
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const admin = await runCli(
    ["explorer", "get", COLLECTION_ID, "doc_1", ...TENANT, "--reason", "support ticket 42", "--json"],
    { configDir: directory },
  );
  assert.equal(admin.code, 0, admin.stderr);
  assert.equal(JSON.parse(admin.stdout).documentId, "doc_1");
  neverPrinted(admin, CAPABILITY);
  const issued = api.find(GRANTS, "POST")[0];
  assert.deepEqual(issued.body, {
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: COLLECTION_ID,
    mode: "administrative",
    operations: ["get"],
    applicationUserId: null,
    reason: "support ticket 42",
    durationSeconds: 60,
  });
  const read = api.find(`${EXPLORER}/documents/doc_1`, "GET")[0];
  assert.equal(read.headers["x-mako-explorer-capability"], CAPABILITY);
  assert.equal(read.headers.authorization, "Bearer developer-session-token-0001");
  const revoked = api.find(`${GRANTS}/${GRANT_ID}`, "DELETE");
  assert.equal(revoked.length, 1);
  assert.ok(api.requests.indexOf(revoked[0]) > api.requests.indexOf(read), "revoked after the read");

  const preview = await runCli(
    ["explorer", "get", COLLECTION_ID, "doc_1", ...TENANT, "--as-user", "usr_1", "--duration", "30"],
    { configDir: directory },
  );
  assert.equal(preview.code, 0, preview.stderr);
  assert.match(preview.stdout, /documentId\s+doc_1/u);
  neverPrinted(preview, CAPABILITY);
  const previewGrant = api.find(GRANTS, "POST")[1].body;
  assert.equal(previewGrant.mode, "policy_preview");
  assert.equal(previewGrant.applicationUserId, "usr_1");
  assert.equal(previewGrant.reason, null);
  assert.equal(previewGrant.durationSeconds, 30);
  assert.equal(api.find(`${GRANTS}/${GRANT_ID}`, "DELETE").length, 2);

  const noReason = await runCli(["explorer", "get", COLLECTION_ID, "doc_1", ...TENANT], {
    configDir: directory,
  });
  assert.equal(noReason.code, 2);
  assert.match(noReason.stderr, /--reason/u);
  assert.equal(api.find(GRANTS, "POST").length, 2, "nothing is issued for a usage error");
  assert.deepEqual(api.unhandled, []);
});

test("a failing explorer read still revokes the grant and keeps the capability out of the output", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: explorerHandler((request) =>
        request.path === `${EXPLORER}/documents/doc_missing`
          ? apiError("not_found", "no such document", 404)
          : undefined,
      ),
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const result = await runCli(
    ["explorer", "get", COLLECTION_ID, "doc_missing", ...TENANT, "--reason", "looking for it"],
    { configDir: directory },
  );
  assert.equal(result.code, 4);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /not_found/u);
  neverPrinted(result, CAPABILITY);
  assert.equal(api.find(`${GRANTS}/${GRANT_ID}`, "DELETE").length, 1, "revoked despite the failure");
});

test("browse, plan, query, history, and simulate each scope their grant to one operation", async (t) => {
  const page = (items, nextCursor) => ({ items, nextCursor, snapshot: "snap_1", exhausted: nextCursor === null });
  const api = await startMockApi(
    authHandler({
      fallback: explorerHandler((request) => {
        if (request.method === "POST" && request.path === `${EXPLORER}/browse`) {
          return request.body.cursor === null
            ? { status: 200, json: page([explorerDocument()], "c1") }
            : { status: 200, json: page([explorerDocument({ documentId: "doc_2" })], null) };
        }
        if (request.method === "POST" && request.path === `${EXPLORER}/query/plan`) {
          return {
            status: 200,
            json: { supported: true, indexName: "by_title", effectiveOrder: [], effectiveLimit: 5, queryFingerprint: "fp", requiredIndex: null },
          };
        }
        if (request.method === "POST" && request.path === `${EXPLORER}/query`) {
          return { status: 200, json: page([explorerDocument()], null) };
        }
        if (request.method === "GET" && request.path === `${EXPLORER}/documents/doc_1/history`) {
          return {
            status: 200,
            json: [{ revision: "1-abc", schemaVersion: 3, commitPosition: 7, committedAtUnixSeconds: 1, deleted: false, retainedUntilUnixSeconds: null }],
          };
        }
        if (request.method === "POST" && request.path === `${EXPLORER}/simulate`) {
          return { status: 200, json: { allowed: true, schemaValid: true, diagnostics: [], wouldConflict: false } };
        }
        return undefined;
      }),
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const reason = ["--reason", "routine inspection"];
  const query = JSON.stringify({ predicates: [{ field: "title", operator: "equal", value: "hello" }], sort: [] });
  const mutation = JSON.stringify({ kind: "update", documentId: "doc_1", expectedRevision: "1-abc", schemaVersion: 3, content: { title: "changed" } });

  const browse = await runCli(["explorer", "browse", COLLECTION_ID, ...TENANT, ...reason, "--limit", "1", "--all", "--json"], { configDir: directory });
  assert.equal(browse.code, 0, browse.stderr);
  assert.deepEqual(JSON.parse(browse.stdout).items.map((item) => item.documentId), ["doc_1", "doc_2"]);
  assert.deepEqual(api.find(`${EXPLORER}/browse`, "POST")[0].body, { limit: 1, cursor: null, includeRetainedTombstones: false });

  const plan = await runCli(["explorer", "plan", COLLECTION_ID, ...TENANT, ...reason, "--query", query, "--limit", "5"], { configDir: directory });
  assert.equal(plan.code, 0, plan.stderr);
  assert.match(plan.stdout, /indexName\s+by_title/u);
  assert.deepEqual(api.find(`${EXPLORER}/query/plan`, "POST")[0].body, {
    predicates: [{ field: "title", operator: "equal", value: "hello" }],
    sort: [],
    limit: 5,
    cursor: null,
  });

  const queried = await runCli(["explorer", "query", COLLECTION_ID, ...TENANT, "--as-user", "usr_1", "--query", "-"], { configDir: directory, stdin: query });
  assert.equal(queried.code, 0, queried.stderr);
  assert.match(queried.stdout, /^documentId\s+revision/u);
  assert.match(queried.stdout, /doc_1/u);

  const history = await runCli(["explorer", "history", COLLECTION_ID, "doc_1", ...TENANT, ...reason], { configDir: directory });
  assert.equal(history.code, 0, history.stderr);
  assert.match(history.stdout, /^revision\s+schemaVersion\s+commitPosition/u);
  assert.match(history.stdout, /1-abc/u);

  const simulated = await runCli(["explorer", "simulate", COLLECTION_ID, ...TENANT, ...reason, "--mutation", mutation, "--json"], { configDir: directory });
  assert.equal(simulated.code, 0, simulated.stderr);
  assert.equal(JSON.parse(simulated.stdout).allowed, true);
  const sent = api.find(`${EXPLORER}/simulate`, "POST")[0].body;
  assert.equal(sent.kind, "update");
  assert.equal(sent.expectedRevision, "1-abc");
  assert.ok(sent.idempotencyKey.length >= 16, "an idempotency key is generated");

  assert.deepEqual(
    api.find(GRANTS, "POST").map((request) => request.body.operations),
    [["browse"], ["plan"], ["query"], ["history"], ["simulate"]],
  );
  assert.equal(api.find(`${GRANTS}/${GRANT_ID}`, "DELETE").length, 5);
  for (const result of [browse, plan, queried, history, simulated]) neverPrinted(result, CAPABILITY);
  assert.deepEqual(api.unhandled, []);
});

test("explorer mutate refuses without --yes, and with it commits under a mutate-only grant", async (t) => {
  let conflict = false;
  const api = await startMockApi(
    authHandler({
      fallback: explorerHandler((request) => {
        if (request.method === "POST" && request.path === `${EXPLORER}/mutate`) {
          return conflict
            ? { status: 200, json: { committed: false, document: null, conflict: explorerDocument({ revision: "2-def" }), auditReference: "aud_2" } }
            : { status: 200, json: { committed: true, document: explorerDocument({ revision: "2-abc" }), conflict: null, auditReference: "aud_1" } };
        }
        return undefined;
      }),
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const mutation = JSON.stringify({ kind: "delete", documentId: "doc_1", expectedRevision: "1-abc", idempotencyKey: "delete-doc_1-attempt-0001" });
  const common = ["explorer", "mutate", COLLECTION_ID, ...TENANT, "--reason", "remove test data", "--mutation", mutation, "--schema-version", "3"];

  const refused = await runCli(common, { configDir: directory });
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(GRANTS, "POST").length, 0, "no grant is issued before confirmation");

  const done = await runCli([...common, "--yes", "--json"], { configDir: directory });
  assert.equal(done.code, 0, done.stderr);
  assert.equal(JSON.parse(done.stdout).committed, true);
  neverPrinted(done, CAPABILITY);
  assert.deepEqual(api.find(GRANTS, "POST")[0].body.operations, ["mutate"]);
  const sent = api.find(`${EXPLORER}/mutate`, "POST")[0];
  assert.equal(sent.headers["x-mako-explorer-capability"], CAPABILITY);
  assert.equal(sent.headers["idempotency-key"], "delete-doc_1-attempt-0001");
  assert.deepEqual(sent.body, {
    kind: "delete",
    documentId: "doc_1",
    schemaVersion: 3,
    expectedRevision: "1-abc",
    idempotencyKey: "delete-doc_1-attempt-0001",
    content: null,
  });
  assert.equal(api.find(`${GRANTS}/${GRANT_ID}`, "DELETE").length, 1);

  conflict = true;
  const stale = await runCli([...common, "--yes", "--json"], { configDir: directory });
  assert.equal(stale.code, 5, stale.stderr);
  assert.equal(JSON.parse(stale.stdout).conflict.revision, "2-def", "the conflict is printed for comparison");
  assert.match(stale.stderr, /not committed/u);
  assert.equal(api.find(`${GRANTS}/${GRANT_ID}`, "DELETE").length, 2);
});

test("data jobs list, get, and cancel", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === JOBS) {
          return { status: 200, json: { items: [dataJob(), dataJob({ jobId: "job_import01", kind: "import", state: "awaiting_upload", conflictStrategy: "upsert" })] } };
        }
        if (request.method === "GET" && request.path === JOB) return { status: 200, json: dataJob({ state: "running" }) };
        if (request.method === "POST" && request.path === `${JOB}/actions/cancel`) return { status: 200, json: dataJob({ state: "cancelling" }) };
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const list = await runCli(["data", "jobs", "list", ...TENANT], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.match(list.stdout, /^jobId\s+kind\s+state\s+collectionId\s+strategy\s+createdAt\n/u);
  assert.match(list.stdout, /job_import01\s+import\s+awaiting_upload\s+col_abcdefgh\s+upsert/u);

  const get = await runCli(["data", "jobs", "get", JOB_ID, ...TENANT, "--json"], { configDir: directory });
  assert.equal(get.code, 0, get.stderr);
  assert.equal(JSON.parse(get.stdout).state, "running");

  const refused = await runCli(["data", "jobs", "cancel", JOB_ID, ...TENANT], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.equal(api.find(`${JOB}/actions/cancel`, "POST").length, 0);

  const cancelled = await runCli(["data", "jobs", "cancel", JOB_ID, ...TENANT, "--yes", "--json"], { configDir: directory });
  assert.equal(cancelled.code, 0, cancelled.stderr);
  assert.equal(JSON.parse(cancelled.stdout).state, "cancelling");
  assert.equal(api.find(`${JOB}/actions/cancel`, "POST").length, 1);
});

const EXPORT_LINES = '{"id":"doc_1","title":"hello"}\n{"id":"doc_2","title":"wörld"}\n';

/** An export that is running on the first poll and succeeded on the next. */
function exportHandler({ digest = `sha256:${sha256(EXPORT_LINES)}`, ready = true, responseDigest } = {}) {
  let polls = 0;
  return (request) => {
    if (request.method === "POST" && request.path === JOBS) return { status: 201, json: dataJob() };
    if (request.method === "GET" && request.path === JOB) {
      polls += 1;
      const done = ready && polls > 1;
      return {
        status: 200,
        json: dataJob({
          state: done ? "succeeded" : "running",
          progress: { processed: 0, committed: 0, failed: 0, skipped: 0, exported: done ? 2 : 1, bytes: done ? EXPORT_LINES.length : 10 },
          manifest: done ? manifest({ digest }) : null,
        }),
      };
    }
    if (request.method === "POST" && request.path === `${JOB}/artifact-grants/download`) {
      return {
        status: 201,
        json: { jobId: JOB_ID, method: "GET", url: `http://127.0.0.1:${new URL(request.headers.origin).port}${JOB}/artifact?grant=${ARTIFACT_GRANT}`, digest, expiresAtUnixSeconds: 1_700_000_300 },
      };
    }
    if (request.method === "GET" && request.path === `${JOB}/artifact`) {
      if (request.query.grant !== ARTIFACT_GRANT) return apiError("permission_denied", "artifact grant is invalid", 403);
      return {
        status: 200,
        text: EXPORT_LINES,
        headers: { "content-type": "application/x-ndjson", Digest: responseDigest ?? `sha-256=${sha256(EXPORT_LINES)}` },
      };
    }
    return undefined;
  };
}

test("data export creates the job, waits, downloads the artifact to a file, and prints the job id", async (t) => {
  const api = await startMockApi(authHandler({ fallback: exportHandler() }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const output = join(directory, "export.jsonl");

  const result = await runCli(["data", "export", "--collection", COLLECTION_ID, "--output", output, ...TENANT, "--json"], {
    configDir: directory,
    env: WAIT_FAST,
  });
  assert.equal(result.code, 0, result.stderr);
  assert.equal(await readFile(output, "utf8"), EXPORT_LINES);
  await assert.rejects(stat(`${output}.part`), "no partial file remains");
  const parsed = JSON.parse(result.stdout);
  assert.equal(parsed.job.state, "succeeded");
  assert.equal(parsed.output, output);
  assert.equal(parsed.bytes, Buffer.byteLength(EXPORT_LINES));
  assert.equal(parsed.digest, `sha256:${sha256(EXPORT_LINES)}`);
  assert.match(result.stderr, /^job job_abcdefgh\n/u);
  assert.match(result.stderr, /resume with: mako-cloud data export --job job_abcdefgh/u);
  assert.match(result.stderr, /job job_abcdefgh: running/u);
  assert.match(result.stderr, /job job_abcdefgh: succeeded/u);
  assert.equal(result.stderr.includes(ARTIFACT_GRANT), false, "the grant URL is never printed");

  const created = api.find(JOBS, "POST")[0];
  assert.deepEqual(created.body, { kind: "export", collectionId: COLLECTION_ID, conflictStrategy: null });
  assert.ok(created.headers["idempotency-key"], "job creation is idempotent");
  const download = api.find(`${JOB}/artifact`, "GET")[0];
  assert.equal(download.headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(download.headers.origin, api.endpoint);
  assert.deepEqual(api.unhandled, []);

  const toStdout = await runCli(["data", "export", "--job", JOB_ID, "--output", "-", ...TENANT], { configDir: directory, env: WAIT_FAST });
  assert.equal(toStdout.code, 0, toStdout.stderr);
  assert.equal(toStdout.stdout, EXPORT_LINES, "stdout is exactly the artifact");
  assert.equal(api.find(JOBS, "POST").length, 1, "--job resumes without creating another job");
});

test("data export refuses a mismatched artifact and keeps no file", async (t) => {
  const api = await startMockApi(authHandler({ fallback: exportHandler({ digest: `sha256:${"0".repeat(64)}` }) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const output = join(directory, "bad.jsonl");
  const result = await runCli(["data", "export", "--collection", COLLECTION_ID, "--output", output, ...TENANT], {
    configDir: directory,
    env: WAIT_FAST,
  });
  assert.equal(result.code, 1);
  assert.match(result.stderr, /does not match/u);
  await assert.rejects(stat(output));
  await assert.rejects(stat(`${output}.part`));
});

test("data export exits 6 when the job does not finish before --timeout", async (t) => {
  const api = await startMockApi(authHandler({ fallback: exportHandler({ ready: false }) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const output = join(directory, "late.jsonl");
  const result = await runCli(["data", "export", "--collection", COLLECTION_ID, "--output", output, ...TENANT, "--timeout", "1"], {
    configDir: directory,
    env: { MAKO_WAIT_INTERVAL_MS: "20" },
  });
  assert.equal(result.code, 6, result.stderr);
  assert.match(result.stderr, /^job job_abcdefgh\n/u, "the job id is printed before waiting");
  assert.match(result.stderr, /timed out waiting; last observed: job job_abcdefgh: running/u);
  assert.equal(api.find(`${JOB}/artifact-grants/download`, "POST").length, 0);
  await assert.rejects(stat(output));
});

const IMPORT_LINES = '{"id":"doc_1","title":"one"}\n{"id":"doc_2","title":"two"}\n';

/** An import job whose state advances with each call, as the control plane's does. */
function importHandler() {
  const state = { current: "none", manifest: null, uploaded: null };
  const job = (overrides = {}) =>
    dataJob({ kind: "import", state: state.current, conflictStrategy: "upsert", manifest: state.manifest, ...overrides });
  return (request) => {
    if (request.method === "POST" && request.path === JOBS) {
      state.current = "awaiting_upload";
      state.manifest = null;
      return { status: 201, json: job({ conflictStrategy: request.body.conflictStrategy }) };
    }
    if (request.method === "GET" && request.path === JOB) {
      if (state.current === "queued") state.current = "succeeded";
      return { status: 200, json: job() };
    }
    if (request.method === "POST" && request.path === `${JOB}/artifact-grants/upload`) {
      if (state.current !== "awaiting_upload") return apiError("conflict", "job is not awaiting an upload", 409);
      return { status: 201, json: { jobId: JOB_ID, method: "PUT", url: `${JOB}/artifact?grant=${ARTIFACT_GRANT}`, digest: null, expiresAtUnixSeconds: 1_700_000_300 } };
    }
    if (request.method === "PUT" && request.path === `${JOB}/artifact`) {
      if (request.query.grant !== ARTIFACT_GRANT) return apiError("permission_denied", "artifact grant is invalid", 403);
      if (request.headers.digest !== `sha-256=${sha256(request.text)}`) return apiError("invalid_request", "digest mismatch", 400);
      state.uploaded = request.text;
      state.current = "dry_run";
      return { status: 202, json: job() };
    }
    if (request.method === "POST" && request.path === `${JOB}/actions/dry-run`) {
      if (state.current !== "dry_run") return apiError("conflict", "job is not awaiting a dry run", 409);
      if (request.body.uploadDigest !== `sha256:${sha256(state.uploaded)}`) return apiError("invalid_request", "upload digest mismatch", 400);
      state.current = "awaiting_confirmation";
      state.manifest = manifest({ schemaVersion: request.body.schemaVersion, byteCount: state.uploaded.length });
      return { status: 200, json: job({ progress: { processed: 2, committed: 0, failed: 1, skipped: 0, exported: 0, bytes: state.uploaded.length }, errors: ["row 2: title must be a string"] }) };
    }
    if (request.method === "POST" && request.path === `${JOB}/actions/confirm`) {
      if (state.current !== "awaiting_confirmation") return apiError("conflict", "job is not awaiting confirmation", 409);
      if (request.body.expectedManifestDigest !== state.manifest.digest) return apiError("precondition_failed", "manifest changed", 412);
      state.current = "queued";
      return { status: 200, json: job() };
    }
    return undefined;
  };
}

test("data import uploads with the digest, shows the dry run, and confirms only with --yes", async (t) => {
  const api = await startMockApi(authHandler({ fallback: importHandler() }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const input = join(directory, "rows.jsonl");
  await writeFile(input, IMPORT_LINES);
  const common = ["data", "import", "--collection", COLLECTION_ID, "--input", input, "--strategy", "upsert", "--schema-version", "3", ...TENANT];
  const confirms = () => api.find(`${JOB}/actions/confirm`, "POST").length;

  const refused = await runCli(common, { configDir: directory, env: WAIT_FAST });
  assert.equal(refused.code, 2, refused.stderr);
  assert.equal(refused.stdout, "");
  assert.match(refused.stderr, /^job job_abcdefgh\n/u);
  assert.match(refused.stderr, /resume with: mako-cloud data import --job job_abcdefgh/u);
  assert.match(refused.stderr, new RegExp(`uploaded ${Buffer.byteLength(IMPORT_LINES)} bytes`, "u"));
  assert.match(refused.stderr, /dry run for import job job_abcdefgh/u);
  assert.match(refused.stderr, /2 rows/u);
  assert.match(refused.stderr, /error: row 2: title must be a string/u);
  assert.match(refused.stderr, /refusing to confirm import job_abcdefgh without --yes/u);
  assert.equal(confirms(), 0, "no confirmation is sent");
  assert.equal(refused.stderr.includes(ARTIFACT_GRANT), false, "the grant URL is never printed");

  const upload = api.find(`${JOB}/artifact`, "PUT")[0];
  assert.equal(upload.text, IMPORT_LINES);
  assert.equal(upload.headers.digest, `sha-256=${sha256(IMPORT_LINES)}`);
  assert.equal(upload.headers["content-type"], "application/x-ndjson");
  assert.equal(upload.headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(upload.query.grant, ARTIFACT_GRANT);
  assert.deepEqual(api.find(JOBS, "POST")[0].body, { kind: "import", collectionId: COLLECTION_ID, conflictStrategy: "upsert" });
  assert.deepEqual(api.find(`${JOB}/actions/dry-run`, "POST")[0].body, { uploadDigest: `sha256:${sha256(IMPORT_LINES)}`, schemaVersion: 3 });

  const resumed = await runCli(["data", "import", "--job", JOB_ID, ...TENANT, "--yes", "--wait", "--json"], { configDir: directory, env: WAIT_FAST });
  assert.equal(resumed.code, 0, resumed.stderr);
  assert.equal(confirms(), 1);
  assert.deepEqual(api.find(`${JOB}/actions/confirm`, "POST")[0].body, {
    expectedManifestDigest: `sha256:${"m".repeat(64)}`,
    acknowledgePartialImportCancellation: true,
  });
  assert.equal(JSON.parse(resumed.stdout).state, "succeeded", "--wait follows the confirmed job to its end");
  assert.equal(api.find(`${JOB}/artifact`, "PUT").length, 1, "resuming after the dry run uploads nothing");

  const dryRunOnly = await runCli([...common, "--dry-run-only", "--json"], { configDir: directory, env: WAIT_FAST });
  assert.equal(dryRunOnly.code, 0, dryRunOnly.stderr);
  assert.equal(JSON.parse(dryRunOnly.stdout).state, "awaiting_confirmation");
  assert.match(dryRunOnly.stderr, /confirm later with: mako-cloud data import --job job_abcdefgh --yes/u);
  assert.equal(confirms(), 1, "--dry-run-only never confirms");

  const typed = await runCli(common, { configDir: directory, env: WAIT_FAST, isTTY: true, stdin: `${JOB_ID}\n` });
  assert.equal(typed.code, 0, typed.stderr);
  assert.match(typed.stderr, /Type "job_abcdefgh" to confirm import/u);
  assert.equal(confirms(), 2, "typing the job id confirms");
  assert.match(typed.stdout, /state\s+queued/u);
  assert.deepEqual(api.unhandled, []);
});

test("data import needs a real input file and a known strategy before anything is sent", async (t) => {
  const api = await startMockApi(authHandler({ fallback: importHandler() }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const missing = await runCli(
    ["data", "import", "--collection", COLLECTION_ID, "--input", join(directory, "nope.jsonl"), "--strategy", "upsert", "--schema-version", "3", ...TENANT],
    { configDir: directory },
  );
  assert.equal(missing.code, 2);
  assert.match(missing.stderr, /does not exist/u);
  const badStrategy = await runCli(
    ["data", "import", "--collection", COLLECTION_ID, "--input", join(directory, "nope.jsonl"), "--strategy", "overwrite", ...TENANT],
    { configDir: directory },
  );
  assert.equal(badStrategy.code, 2);
  assert.match(badStrategy.stderr, /--strategy must be one of/u);
  assert.equal(api.find(JOBS, "POST").length, 1, "the job is created before the input is checked only once the strategy is valid");
});
