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
  },
  build: {
    outDir: "web-dist",
    emptyOutDir: true,
  },
});
