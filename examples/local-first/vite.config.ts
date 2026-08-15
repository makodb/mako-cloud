import { defineConfig } from "vite";

/**
 * A deployed Mako serves the application and the API from one origin behind a
 * reverse proxy, and the data plane emits no CORS headers, so a browser can only
 * reach it same-origin. When `MAKO_LIVE_ENDPOINT` is set the dev server proxies
 * `/v1` to that deployment, reproducing the deployed topology rather than
 * loosening the server to suit a test.
 */
const liveEndpoint = process.env.MAKO_LIVE_ENDPOINT;

export default defineConfig({
  server:
    liveEndpoint === undefined
      ? {}
      : {
          proxy: {
            "/v1": {
              target: liveEndpoint,
              changeOrigin: false,
              // The live pull stream is server-sent events; it must not be buffered.
              configure: (proxy) => {
                proxy.on("proxyRes", (proxyRes) => {
                  if (proxyRes.headers["content-type"]?.includes("text/event-stream")) {
                    proxyRes.headers["cache-control"] = "no-cache";
                  }
                });
              },
            },
          },
        },
});
