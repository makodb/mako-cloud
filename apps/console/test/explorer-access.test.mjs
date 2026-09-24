import assert from "node:assert/strict";
import test from "node:test";
import { createExplorerAccess } from "../dist/explorer-access.js";

const grant = (id = "grant-1") => ({
  grantId: id,
  capability: `mx1.test.${id}`,
  expiresAtUnixSeconds: Math.floor(Date.now() / 1000) + 300,
});

test("concurrent document actions share one pending grant and closing revokes it once", async () => {
  let finish;
  let issued = 0;
  const revoked = [];
  const access = createExplorerAccess({
    issueExplorerGrant: () => {
      issued += 1;
      return new Promise((resolve) => { finish = resolve; });
    },
    revokeExplorerGrant: async (...args) => { revoked.push(args); },
  }, "project", "environment", "collection");
  const first = access.getGrant();
  const second = access.getGrant();
  const result = grant();
  finish(result);
  assert.deepEqual(await Promise.all([first, second]), [result, result]);
  assert.equal(await access.getGrant(), result);
  assert.equal(issued, 1);
  access.close();
  access.close();
  assert.deepEqual(revoked, [["project", "environment", "grant-1"]]);
  await assert.rejects(access.getGrant(), /collection changed/u);
});

test("a grant arriving after scope disposal is revoked and never returned", async () => {
  let finish;
  const revoked = [];
  const access = createExplorerAccess({
    issueExplorerGrant: () => new Promise((resolve) => { finish = resolve; }),
    revokeExplorerGrant: async (...args) => { revoked.push(args); },
  }, "old-project", "old-environment", "old-collection");
  const pending = access.getGrant();
  access.close();
  finish(grant());
  await assert.rejects(pending, /collection changed/u);
  assert.deepEqual(revoked, [["old-project", "old-environment", "grant-1"]]);
});

test("an expired grant from the service is refused without an issuance loop", async () => {
  let issued = 0;
  const revoked = [];
  const access = createExplorerAccess({
    issueExplorerGrant: async () => {
      issued += 1;
      return { ...grant(), expiresAtUnixSeconds: 1 };
    },
    revokeExplorerGrant: async (...args) => { revoked.push(args); },
  }, "project", "environment", "collection");
  await assert.rejects(access.getGrant(), /could not be renewed/u);
  assert.equal(issued, 1);
  assert.equal(revoked.length, 1);
  access.close();
});

test("permission failures propagate and a later explicit action rechecks access", async () => {
  let issued = 0;
  const denial = new Error("forbidden");
  const access = createExplorerAccess({
    issueExplorerGrant: async () => {
      issued += 1;
      if (issued === 1) throw denial;
      return grant();
    },
    revokeExplorerGrant: async () => {},
  }, "project", "environment", "collection");
  await assert.rejects(access.getGrant(), (error) => error === denial);
  assert.equal(issued, 1);
  assert.equal((await access.getGrant()).grantId, "grant-1");
  assert.equal(issued, 2);
  access.close();
});

test("invalidation drops the old credential even if revocation is unavailable", async () => {
  let issued = 0;
  const access = createExplorerAccess({
    issueExplorerGrant: async () => grant(`grant-${++issued}`),
    revokeExplorerGrant: async () => { throw new Error("unavailable"); },
  }, "project", "environment", "collection");
  assert.equal((await access.getGrant()).grantId, "grant-1");
  access.invalidate();
  assert.equal((await access.getGrant()).grantId, "grant-2");
  access.close();
});
