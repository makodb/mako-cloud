// The design-system validator refuses a raw control and names the file.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { ALLOWED, findRawControls, main, rawControlsIn } from "../validate-ui-kit.js";

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

test("the kit's own components and a file picker pass; an allow-list can excuse a path", () => {
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

    "packages/ui/src/components/button.tsx":
      'export const Button = () => <button type="button" />;',
  });
  try {
    // Nothing is excused today; the mechanism is still here for the next
    // surface that adopts the kit screen by screen.
    assert.equal(ALLOWED.size, 0);
    assert.deepEqual(findRawControls(root), []);
    const log = collectingLog();
    assert.equal(main(root, log), 0);
    assert.match(log.lines.log[0], /no raw controls/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a control named in a comment or a string is not a control", () => {
  const root = fixture({
    "examples/rational/src/ui/prose.tsx": [
      "// The kit replaced every raw <button> on this screen.",
      "/**",
      " * A checkbox is a <button> to the browser, so the kit styles it as one.",
      " */",
      'const HELP = "use <Input> instead of <input>";',
      "export const Prose = () => <p>{HELP}</p>;",
    ].join("\n"),
  });
  try {
    assert.deepEqual(findRawControls(root), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("prose, comments and strings never look like controls, and real ones are still found", () => {
  // The apostrophe in "Don't" once blinded the whole rest of the file.
  const source = [
    "// A screen may not render its own <button>.",
    "/** A checkbox is a <button> to the browser. */",
    'const HELP = "use <Input> instead of <input>";',
    "export const A = () => (",
    "  <div>",
    "    <p>Nothing here yet. Don't worry — add one.</p>",
    "    <Button>Fine</Button>",
    '    <input type="file" accept=".csv" />',
    '    <select name="mode"><option /></select>',
    "  </div>",
    ");",
  ].join("\n");
  // Only the select is rendered: the rest are prose, a comment, a string, a
  // kit component, and the file picker the kit leaves to the application.
  assert.deepEqual(rawControlsIn("screen.tsx", source), [{ line: 9, element: "select" }]);
});

test("a missing source tree is not a failure", () => {
  const root = fixture({});
  try {
    assert.deepEqual(findRawControls(root), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
