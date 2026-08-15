#!/usr/bin/env node

import { parseServeArguments, runLocalServe } from "./serve.js";

async function main(): Promise<void> {
  const config = parseServeArguments(process.argv.slice(2), process.cwd());
  await runLocalServe(config);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : "local function serve failed";
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
});
