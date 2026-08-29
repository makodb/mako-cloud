// The runtime ships this package to a user worker as a first-party module, so
// the checked-in copy next to the main worker must be exactly what the current
// source emits -- otherwise a deployed function would import a stale SDK.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { RUNTIME_MODULE, renderRuntimeModule } from "../scripts/build-runtime-module.mjs";

test("the runtime module shipped with the main worker matches the built SDK", () => {
  assert.equal(
    readFileSync(RUNTIME_MODULE, "utf8"),
    renderRuntimeModule(),
    "packages/cli/runtime/main/edge-sdk-source.ts is stale; run `npm run build:runtime-module -w @mako-cloud/edge-sdk`",
  );
});

test("the module the worker loads declares the documented entry points", () => {
  const source = readFileSync(RUNTIME_MODULE, "utf8");
  for (const name of [
    "createFunctionClient",
    "createFunctionClientFromRequest",
    "createServiceClient",
    "MakoEdgeSdkError",
    "MakoCallerIdentityRequiredError",
  ]) {
    assert.ok(
      source.includes(`export function ${name}`) || source.includes(`export class ${name}`),
      `${name} is missing from the runtime module`,
    );
  }
  // A worker may read nothing outside its own directory and has no network,
  // so any import at all would be a boot failure.
  assert.equal(/^\s*(?:import|export)\s[^;]*\sfrom\s/mu.test(source), false);
});
