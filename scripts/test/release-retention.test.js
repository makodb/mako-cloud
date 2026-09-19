// `mako-release prune` removes installed releases nobody can select any more.
// The script is exercised against a private release root: fake releases carry
// only the manifest fields pruning reads, since pruning never verifies a
// release it is about to delete -- it only refuses to delete a selected one.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const script = resolve(import.meta.dirname, "../../infra/ansible/roles/runtime/files/mako-release");
const operation = resolve(
  import.meta.dirname,
  "../../infra/ansible/roles/runtime/files/mako-release-operation",
);

function digest(index) {
  return String(index).padStart(64, "0");
}

function releaseRoot(builds) {
  const root = mkdtempSync(join(tmpdir(), "mako-release-retention-"));
  mkdirSync(join(root, "releases"));
  for (const [index, generatedAt] of builds.entries()) {
    const directory = join(root, "releases", digest(index));
    mkdirSync(join(directory, "bin"), { recursive: true });
    writeFileSync(join(directory, "manifest.json"), JSON.stringify({ generatedAt }));
    writeFileSync(join(directory, "bin", "mako-data-plane"), "binary");
  }
  return root;
}

function select(root, name, index) {
  symlinkSync(`releases/${digest(index)}`, join(root, name));
}

function prune(root, ...args) {
  return spawnSync("bash", [script, "--root", root, "prune", ...args], { encoding: "utf8" });
}

function installed(root) {
  return readdirSync(join(root, "releases"))
    .filter((name) => /^[0-9a-f]{64}$/u.test(name))
    .map((name) => Number.parseInt(name, 10))
    .sort((left, right) => left - right);
}

test("prune keeps the selected, last-known-good, and newest releases and removes the rest", () => {
  // Ten releases built a day apart; release 9 is the newest.
  const root = releaseRoot(
    Array.from(
      { length: 10 },
      (_, index) => `2026-09-${String(index + 1).padStart(2, "0")}T00:00:00Z`,
    ),
  );
  // An old release is selected and an even older one is last-known-good.
  select(root, "current", 3);
  select(root, "last-known-good", 1);
  try {
    const result = prune(root, "--keep", "3");
    assert.equal(result.status, 0, result.stderr);
    // Newest three (9, 8, 7) plus the two selectors (3, 1).
    assert.deepEqual(installed(root), [1, 3, 7, 8, 9]);
    assert.match(result.stdout, /retained 5 releases, pruned 5/u);
    assert.equal(result.stdout.match(/^pruned release: /gmu).length, 5);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("prune counts selected releases among the newest and never removes them", () => {
  const root = releaseRoot([
    "2026-09-01T00:00:00Z",
    "2026-09-02T00:00:00Z",
    "2026-09-03T00:00:00Z",
  ]);
  select(root, "current", 2);
  select(root, "last-known-good", 1);
  try {
    const result = prune(root, "--keep", "1");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(installed(root), [1, 2]);
    assert.match(result.stdout, /retained 2 releases, pruned 1/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("prune leaves staging directories and unrelated entries alone", () => {
  const root = releaseRoot(["2026-09-01T00:00:00Z", "2026-09-02T00:00:00Z"]);
  mkdirSync(join(root, "releases", ".staging-abc-123"));
  writeFileSync(join(root, "releases", "notes.txt"), "not a release");
  select(root, "current", 1);
  try {
    const result = prune(root, "--keep", "1");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(installed(root), [1]);
    const entries = readdirSync(join(root, "releases")).sort();
    assert.ok(entries.includes(".staging-abc-123"), "staging directory was removed");
    assert.ok(entries.includes("notes.txt"), "unrelated entry was removed");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("prune refuses a non-positive retention and an unsafe root", () => {
  const root = releaseRoot(["2026-09-01T00:00:00Z"]);
  try {
    assert.equal(prune(root, "--keep", "0").status, 2);
    assert.equal(prune(root, "--keep", "many").status, 2);
    assert.equal(prune(root, "--keep").status, 2);
    assert.equal(installed(root).length, 1);
    const unsafe = spawnSync("bash", [script, "--root", "/opt", "prune"], { encoding: "utf8" });
    assert.equal(unsafe.status, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a release switch prunes only after the operation is recorded", () => {
  // The operation script is not runnable outside the guest; assert its order
  // of events, which is what keeps pruning housekeeping rather than part of
  // the fail-closed switch.
  const source = spawnSync("cat", [operation], { encoding: "utf8" }).stdout;
  const recorded = source.indexOf(
    'record_operation "$kind" "$current" "$target" "$started" "$completed"',
  );
  const cleared = source.indexOf('trap - EXIT\n  echo "$kind complete');
  const pruned = source.indexOf("prune_releases || true");
  assert.ok(
    recorded > 0 && cleared > recorded && pruned > cleared,
    "pruning must follow the recorded switch",
  );
  assert.match(source, /mako-release prune --keep "\$release_retention"/u);
  assert.match(source, /^release_retention=5$/mu);
});
