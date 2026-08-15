import { defineConfig } from "@playwright/test";

/**
 * Drives the reference application against a running Mako deployment. The
 * default suite (playwright.config.ts) runs the same scenarios against the
 * in-browser fake; this one proves the server implements the other half.
 */
export default defineConfig({
  testDir: "./test-live",
  testMatch: /.*\.spec\.ts/,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  globalSetup: "./test-live/global-setup.ts",
  globalTeardown: "./test-live/global-teardown.ts",
  use: { trace: "retain-on-failure" },
});
