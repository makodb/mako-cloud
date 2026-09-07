import { defineConfig } from "@playwright/test";

/**
 * Several suites may run side by side on one machine (one per screen group
 * while the screens are re-skinned); each takes its own port from the
 * environment so their dev servers do not fight over one.
 */
const port = Number(process.env.CONSOLE_TEST_PORT ?? "4174");

export default defineConfig({
  testDir: "./test-e2e",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npm run dev -- --port ${port} --strictPort`,
    // Ready means the entry module is served, which the dev server does only once
    // it has bundled the dependencies; the bare index answers long before that.
    url: `http://127.0.0.1:${port}/src/main.tsx`,
    reuseExistingServer: false,
    // Cold, the dev server pre-bundles the design system's dependencies first.
    timeout: 120_000,
  },
});
