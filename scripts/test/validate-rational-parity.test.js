import assert from "node:assert/strict";
import test from "node:test";

import { parityCoverage, parseParityMatrix } from "../validate-rational-parity.js";

const HEADER = `| Area | Feature | Status | Where in Rational | Note |
| ---- | ------- | ------ | ----------------- | ---- |`;

function matrix(rows) {
  return `# Parity\n\n${HEADER}\n${rows.join("\n")}\n`;
}

test("coverage counts a partial feature as half and leaves out-of-scope rows out", () => {
  const { rows } = parseParityMatrix(
    matrix([
      "| Accounts | Manual accounts | yes | Accounts | |",
      "| Accounts | Aggregators | partial | Connections | sandbox only |",
      "| Accounts | Valuations | out of scope | — | a valuation service |",
      "| Budget | Forecast | no | — | |",
    ]),
  );
  const coverage = parityCoverage(rows);
  assert.equal(coverage.inScope, 3);
  assert.equal(coverage.score, 1.5);
  assert.equal(coverage.percent, 50);
  assert.deepEqual(coverage.counts, { yes: 1, partial: 1, no: 1, "out of scope": 1 });
});

test("a row with an unknown status, a missing place, or a missing note is refused by line", () => {
  assert.throws(
    () => parseParityMatrix(matrix(["| Budget | Rollover | maybe | Budget | |"])),
    /line 5: status "maybe" is not one of yes, partial, no, out of scope/u,
  );
  assert.throws(
    () => parseParityMatrix(matrix(["| Budget | Rollover | yes | — | |"])),
    /line 5: a feature that is present must say where it lives/u,
  );
  assert.throws(
    () => parseParityMatrix(matrix(["| Budget | Rollover | partial | Budget | |"])),
    /line 5: a partial feature needs a note/u,
  );
  assert.throws(
    () => parseParityMatrix(matrix(["| Budget | Rollover | out of scope | — | |"])),
    /line 5: a out of scope feature needs a note/u,
  );
});

test("a feature listed twice, a malformed row, and a wrong header are all refused", () => {
  assert.throws(
    () =>
      parseParityMatrix(
        matrix([
          "| Budget | Rollover | yes | Budget | |",
          "| Budget | rollover | yes | Budget | |",
        ]),
      ),
    /is listed twice/u,
  );
  assert.throws(
    () => parseParityMatrix(matrix(["| Budget | Rollover | yes |"])),
    /line 5: 3 cells, expected 5/u,
  );
  assert.throws(
    () => parseParityMatrix("| Feature | Status |\n| --- | --- |\n| Rollover | yes |\n"),
    /the header must be/u,
  );
  assert.throws(() => parseParityMatrix("# Nothing here\n"), /no table was found/u);
});

test("the repository's own matrix parses and clears the promised coverage", async () => {
  const { readFileSync } = await import("node:fs");
  const markdown = readFileSync(
    new URL("../../examples/rational/MONARCH-PARITY.md", import.meta.url),
    "utf8",
  );
  const { rows } = parseParityMatrix(markdown);
  assert.ok(rows.length >= 100, `the matrix lists ${rows.length} features`);
  assert.ok(parityCoverage(rows).percent >= 90);
});
