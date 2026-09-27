#!/usr/bin/env node
// Rational is independently maintained. Keep this retired entry point so old
// automation fails safely instead of deleting the application's source tree.
process.stderr.write(
  "Rational export is retired. Develop Rational and Rational Investment in " +
    "https://github.com/shuaimu/rational. No files were changed or published.\n",
);
process.exitCode = 1;
