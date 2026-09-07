// The design-system validator refuses a raw control and names the file.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { ALLOWED, findRawControls, main } from "../validate-ui-kit.js";

function fixture(files) {
  const root = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "validate-ui-kit-"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(root, path, ".."), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

function collectingLog() {
  const lines = { log: [], error: [] };
  return { lines, log: (line) => lines.log.push(line), error: (line) => lines.error.push(line) };
}

test("a raw control in an application source is refused by file and line", () => {
  const root = fixture({
    "examples/rational/src/ui/screen.tsx": [
      'import { Button } from "@mako-cloud/ui";',
      "export function Screen() {",
      "  return (",
      "    <form>",
      '      <button type="submit">Save</button>',
      '      <input value="x" />',
      "      <Button>Fine</Button>",
      "    </form>",
      "  );",
      "}",
    ].join("\n"),
  });
  try {
    const findings = findRawControls(root);
    assert.deepEqual(findings, [
      { file: "examples/rational/src/ui/screen.tsx", line: 5, element: "button" },
      { file: "examples/rational/src/ui/screen.tsx", line: 6, element: "input" },
    ]);
    const log = collectingLog();
    assert.equal(main(root, log), 1);
    assert.match(log.lines.error[0], /screen\.tsx:5: raw <button>/);
    assert.match(log.lines.error.at(-1), /2 raw controls/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the kit's own components, a file picker, and an allow-listed application pass", () => {
  const root = fixture({
    "examples/rational/src/ui/import.tsx": [
      "export function Import() {",
      '  return <input type="file" accept=".csv" />;',
      "}",
    ].join("\n"),
    "examples/rational/src/ui/clean.tsx": [
      'import { Button, Input, NativeSelect } from "@mako-cloud/ui";',
      "export const Clean = () => <><Button /><Input /><NativeSelect><option /></NativeSelect></>;",
    ].join("\n"),
    "apps/console/src/old.tsx": "export const Old = () => <select><option /></select>;",
    "packages/ui/src/components/button.tsx":
      'export const Button = () => <button type="button" />;',
  });
  try {
    assert.ok(ALLOWED.has("apps/console/src/"), "the console is allow-listed until its re-skin");
    assert.deepEqual(findRawControls(root), []);
    const log = collectingLog();
    assert.equal(main(root, log), 0);
    assert.match(log.lines.log[0], /no raw controls/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a missing source tree is not a failure", () => {
  const root = fixture({});
  try {
    assert.deepEqual(findRawControls(root), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
