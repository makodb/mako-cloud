// Every developer-facing operation in the public API has a command, and every
// command's operations exist. Excluded prefixes are printed so a new one is a
// visible decision, never a silent gap.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import yaml from "js-yaml";

import {
  commandOperations,
  commands,
  developerFacingOperations,
  EXCLUDED_PATH_PATTERNS,
  EXCLUDED_PATH_PREFIXES,
} from "../dist/index.js";

const repositoryDirectory = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));

test("every developer-facing operation has a command and every command operation exists", async () => {
  const document = yaml.load(
    await readFile(join(repositoryDirectory, "api/openapi/mako-cloud-v1.yaml"), "utf8"),
  );
  const expected = developerFacingOperations(document);
  const covered = commandOperations(commands);
  const all = new Set();
  for (const item of Object.values(document.paths)) {
    for (const [method, operation] of Object.entries(item)) {
      if (method !== "parameters" && operation?.operationId) all.add(operation.operationId);
    }
  }
  console.log(
    `parity: ${expected.length} developer-facing operations; excluded prefixes ${JSON.stringify(EXCLUDED_PATH_PREFIXES)}; excluded patterns ${EXCLUDED_PATH_PATTERNS.map(String).join(" ")}`,
  );
  const missing = expected.filter((id) => !covered.has(id));
  assert.deepEqual(missing, [], `operations without a command: ${missing.join(", ")}`);
  const unknown = [...covered.keys()].filter((id) => !all.has(id));
  assert.deepEqual(unknown, [], `commands name operations the API does not define: ${unknown.join(", ")}`);
  const paths = new Set();
  for (const command of commands) {
    const key = command.path.join(" ");
    assert.ok(!paths.has(key), `duplicate command ${key}`);
    paths.add(key);
  }
});
