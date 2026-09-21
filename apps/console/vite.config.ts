import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

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
    // Dev-only: the console is a same-origin app in production (the control
    // plane serves the bundle and the /v1 API together). In dev, forward the
    // API to the locally running control plane so sign-in and management work.
    proxy: {
      "/v1": { target: "http://127.0.0.1:8081", changeOrigin: true },
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
