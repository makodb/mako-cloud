import { defineConfig } from "@playwright/test";

/**
 * Drives Rational against a real data plane and control plane started by the
 * global setup, with the app served same-origin through the Vite proxy. The
 * suite uses persistent browser contexts so IndexedDB survives a reload.
 */
export default defineConfig({
  testDir: "./test-live",
  testMatch: /.*\.spec\.ts/,
  timeout: 120_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  globalSetup: "./test-live/global-setup.ts",
  globalTeardown: "./test-live/global-teardown.ts",
  use: { trace: "retain-on-failure" },
});
