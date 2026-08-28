#!/usr/bin/env node
import { processIo, run } from "./cli/run.js";
import { commands } from "./commands/index.js";

run(process.argv.slice(2), processIo(), commands).then(
  (code) => {
    process.exitCode = code;
  },
  (error: unknown) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  },
);
