import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

// Dev-only. In production the reverse proxy serves the console and the /v1 API
// from one origin and splits the API between the two planes; this proxy does
// the same split for a locally running stack. The data plane owns application
// auth, documents, replication, the explorer's document routes, and the
// service routes; everything else under /v1 belongs to the control plane.
const controlPlane = process.env.MAKO_DEV_CONTROL_PLANE_URL ?? "http://127.0.0.1:8081";
const dataPlane = process.env.MAKO_DEV_DATA_PLANE_URL ?? "http://127.0.0.1:8080";
const DATA_PLANE_ROUTES =
  "^/v1/projects/[^/]+/environments/[^/]+/(?:auth/|collections/[^/]+/(?:documents|replication)/|replication/stream$|explorer/collections/|service/)";

export default defineConfig({
  plugins: [tailwindcss()],
  // The design system is a linked workspace package, so its dependencies are
  // not found by the dev server's first scan; naming them here has them
  // pre-bundled up front instead of on the first request.
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
  server: {
    // Suites running side by side write their artefacts next to the sources;
    // a dev server must not reload every page each time one of them does.
    watch: { ignored: ["**/test-results*/**", "**/playwright-report*/**"] },
    // The first matching entry wins, so the data plane's routes come first.
    proxy: {
      [DATA_PLANE_ROUTES]: { target: dataPlane, changeOrigin: true },
      "/v1": { target: controlPlane, changeOrigin: true },
    },
  },
  build: {
    outDir: "web-dist",
    emptyOutDir: true,
    // The libraries change on their own schedule; kept apart from the
    // console's own code so a release does not invalidate every byte.
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          if (!id.includes("node_modules")) return undefined;
          if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return "react";
          if (
            /[\\/]node_modules[\\/](radix-ui|@radix-ui|@floating-ui|aria-hidden|react-remove-scroll)/.test(
              id,
            )
          )
            return "primitives";
          return "vendor";
        },
      },
    },
  },
});
