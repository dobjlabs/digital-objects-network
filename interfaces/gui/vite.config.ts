import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode, command }) => {
  const env = loadEnv(mode, ".", "VITE_");
  return {
    plugins: [react()],
    base: command === "build" ? "./" : "/",
    clearScreen: false,
    server: {
      port: 1420,
      strictPort: true,
      // Keep HTTP and SSE on the UI's origin. The daemon's loopback address
      // belongs to the Vite host, which may differ from the browser's machine.
      proxy: {
        "/api": {
          target: env.VITE_DOBJD_URL || "http://127.0.0.1:7717",
          changeOrigin: true,
          rewrite: (path) => path.replace(/^\/api(?=\/|$)/, ""),
          configure: (proxy) => {
            // The dev proxy is a trusted local client. Forwarded browser
            // origins belong to the UI host, not the daemon's origin.
            proxy.on("proxyReq", (request) => request.removeHeader("origin"));
            // The upstream connection's lifetime must not close the browser's
            // connection while a local port-forwarder is still flushing data.
            proxy.on("proxyRes", (response) => {
              delete response.headers.connection;
            });
          },
        },
      },
    },
  };
});
