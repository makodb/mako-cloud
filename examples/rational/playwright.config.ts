import { defineConfig } from "@playwright/test";

/**
 * Every screen against the in-browser fake backend: fast and hermetic.
 *
 * The fake engages only when no project is configured, and the published
 * repository does configure one -- its site talks to a real project. So this
 * suite asks for the placeholder configuration explicitly rather than
 * depending on the absence of a file.
 */
/**
 * Several suites may run side by side on one machine (one per screen while
 * the screens are built); each takes its own port from the environment so
 * their dev servers do not fight over one.
 */
const port = Number(process.env.RATIONAL_TEST_PORT ?? "4175");

export default defineConfig({
  testDir: "./test",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npm run dev -- --port ${port} --strictPort`,
    env: { RATIONAL_CONFIG: "example" },
    // Ready means the entry module is served, which the dev server does only once
    // it has bundled the dependencies; the bare index answers long before that.
    url: `http://127.0.0.1:${port}/src/main.tsx`,
    reuseExistingServer: false,
    // Cold, the dev server pre-bundles the design system's dependencies first.
    timeout: 120_000,
  },
});
