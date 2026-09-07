import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type ProxyOptions } from "vite";

/**
 * `mako.env.json` is written by `scripts/bootstrap.mjs` and names the tenant
 * the app talks to. The data plane emits no CORS headers, so in development
 * the dev server proxies `/v1` to it and the app calls same-origin — the
 * topology a deployment has behind its reverse proxy. `MAKO_LIVE_ENDPOINT`
 * overrides the proxy target (the live suite points it at a throwaway stack).
 * Without an env file the app runs against its in-browser fake backend.
 */
interface RationalEnvFile {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
  /** Where the edge gateway serves the environment's functions, when deployed. */
  readonly functionsEndpoint?: string | null;
  readonly signIn?: {
    readonly providers: ReadonlyArray<{
      readonly name: string;
      readonly enabled: boolean;
      readonly label?: string;
    }>;
    readonly magicLinks: boolean;
  };
}

function readEnvFile(): RationalEnvFile | null {
  // The wire-mocked browser suite asks for the placeholder configuration
  // explicitly, so a bootstrap file left by pointing this checkout at a real
  // project never turns the hermetic suite into a live one. The published
  // repository's own configuration honours the same switch.
  if (process.env.RATIONAL_CONFIG === "example") return null;
  const path = fileURLToPath(new URL("./mako.env.json", import.meta.url));
  if (!existsSync(path)) return null;
  return JSON.parse(readFileSync(path, "utf8")) as RationalEnvFile;
}

export default defineConfig(({ command }) => {
  const envFile = readEnvFile();
  const liveEndpoint = process.env.MAKO_LIVE_ENDPOINT ?? envFile?.endpoint;
  // The edge gateway is a second origin, and it sends no CORS headers either,
  // so the dev server proxies the function route as well and the app calls
  // both same-origin.
  const functionsEndpoint =
    process.env.MAKO_FUNCTIONS_ENDPOINT ?? envFile?.functionsEndpoint ?? undefined;
  const runtimeEnvironment =
    envFile === null
      ? null
      : {
          ...envFile,
          endpoint: command === "serve" ? "same-origin" : envFile.endpoint,
          functionsEndpoint:
            command === "serve" && functionsEndpoint !== undefined
              ? "same-origin"
              : (envFile.functionsEndpoint ?? null),
        };
  const proxy: Record<string, ProxyOptions> = {};
  if (liveEndpoint !== undefined) {
    proxy["/v1"] = {
      target: liveEndpoint,
      changeOrigin: false,
      configure: (server) => {
        server.on("proxyRes", (proxyRes) => {
          // The live pull stream is server-sent events; never buffer it.
          if (proxyRes.headers["content-type"]?.includes("text/event-stream")) {
            proxyRes.headers["cache-control"] = "no-cache";
          }
        });
      },
    };
  }
  if (functionsEndpoint !== undefined && functionsEndpoint !== null) {
    proxy["^/[^/]+--[^/]+/functions/v1/"] = {
      target: functionsEndpoint,
      changeOrigin: false,
    };
  }
  return {
    plugins: [tailwindcss()],
    // The design system is a linked workspace package, so its dependencies are
    // not found by the dev server's first scan; naming them here has them
    // pre-bundled up front instead of on the first request -- which would
    // otherwise take a minute cold and reload the page mid-way through a test.
    optimizeDeps: {
      include: [
        "react",
        "react-dom",
        "react-dom/client",
        "recharts",
        "radix-ui",
        "lucide-react",
        "clsx",
        "tailwind-merge",
        "class-variance-authority",
      ],
    },
    define: { __RATIONAL_ENV__: JSON.stringify(runtimeEnvironment) },
    build: {
      outDir: "web-dist",
      emptyOutDir: true,
      // The libraries change on their own schedule; kept apart from the app's
      // own code so a release of Rational does not invalidate every byte.
      rollupOptions: {
        output: {
          manualChunks: (id) => {
            if (!id.includes("node_modules")) return undefined;
            if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return "react";
            if (
              /[\\/]node_modules[\\/](recharts|d3-|victory-vendor|internmap|decimal\.js|fast-equals|es-toolkit|reselect|immer|@reduxjs|redux|use-sync-external-store|tiny-invariant)/.test(
                id,
              )
            )
              return "charts";
            if (
              /[\\/]node_modules[\\/](radix-ui|@radix-ui|@floating-ui|aria-hidden|react-remove-scroll)/.test(
                id,
              )
            )
              return "primitives";
            if (/[\\/]node_modules[\\/](?:@[^\\/]+[\\/])?(rxdb|rxjs|dexie)/.test(id))
              return "database";
            return "vendor";
          },
        },
      },
    },
    // Suites running side by side write their artefacts next to the sources;
    // a dev server must not reload every page each time one of them does.
    server: {
      watch: { ignored: ["**/test-results*/**", "**/playwright-report*/**"] },
      ...(Object.keys(proxy).length === 0 ? {} : { proxy }),
    },
  };
});
