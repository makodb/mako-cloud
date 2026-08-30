import { defineConfig } from "@playwright/test";

/**
 * Every screen against the in-browser fake backend: fast and hermetic.
 *
 * The fake engages only when no project is configured, and the published
 * repository does configure one -- its site talks to a real project. So this
 * suite asks for the placeholder configuration explicitly rather than
 * depending on the absence of a file.
 */
export default defineConfig({
  testDir: "./test",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  use: {
    baseURL: "http://127.0.0.1:4175",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "npm run dev -- --port 4175 --strictPort",
    env: { RATIONAL_CONFIG: "example" },
    url: "http://127.0.0.1:4175",
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
