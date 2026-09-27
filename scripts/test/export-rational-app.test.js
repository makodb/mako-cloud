import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../export-rational-app.mjs", import.meta.url));
for (const args of [
  [],
  ["--repo", "git@github.com:shuaimu/rational.git"],
  ["--repo", "https://github.com/shuaimu/rational"],
  ["--dry-run"],
  ["--no-push"],
]) {
  test(`retired export refuses ${JSON.stringify(args)} without invoking git`, () => {
    const result = spawnSync(process.execPath, [script, ...args], {
      encoding: "utf8",
      env: { ...process.env, PATH: "" },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /export is retired/);
    assert.match(result.stderr, /github.com\/shuaimu\/rational/);
    assert.equal(result.stdout, "");
  });
}
test("local checkout contents remain untouched, including independently maintained files", () => {
  const root = mkdtempSync(join(tmpdir(), "retired-rational-"));
  try {
    mkdirSync(join(root, "src"));
    writeFileSync(join(root, "src", "app.ts"), "independent application\n");
    writeFileSync(join(root, "README.md"), "maintained here\n");
    for (const extra of [[], ["--dry-run"], ["--no-push"]]) {
      const result = spawnSync(process.execPath, [script, "--dir", root, ...extra], {
        encoding: "utf8",
        env: { ...process.env, PATH: "" },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /export is retired/);
      assert.deepEqual(readdirSync(root).sort(), ["README.md", "src"]);
      assert.equal(readFileSync(join(root, "src", "app.ts"), "utf8"), "independent application\n");
      assert.equal(readFileSync(join(root, "README.md"), "utf8"), "maintained here\n");
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a missing local destination is never created", () => {
  const root = mkdtempSync(join(tmpdir(), "retired-rational-missing-"));
  try {
    const destination = join(root, "independent application");
    for (const extra of [[], ["--dry-run"], ["--no-push"]]) {
      const result = spawnSync(process.execPath, [script, "--dir", destination, ...extra], {
        encoding: "utf8",
        env: { ...process.env, PATH: "" },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /export is retired/);
      assert.equal(existsSync(destination), false);
      assert.deepEqual(readdirSync(root), []);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an available Git executable is never invoked", () => {
  const root = mkdtempSync(join(tmpdir(), "retired-rational-git-"));
  try {
    const marker = join(root, "git-was-invoked");
    const git = join(root, "git");
    writeFileSync(git, '#!/bin/sh\nprintf invoked > "$MAKO_EXPORT_GIT_MARKER"\nexit 91\n');
    chmodSync(git, 0o700);
    for (const args of [
      [],
      ["--repo", "https://example.invalid/app.git"],
      ["--dir", join(root, "app")],
      ["--dry-run"],
      ["--no-push"],
    ]) {
      const result = spawnSync(process.execPath, [script, ...args], {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, PATH: root, MAKO_EXPORT_GIT_MARKER: marker },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /export is retired/);
      assert.equal(existsSync(marker), false);
      assert.deepEqual(readdirSync(root), ["git"]);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
