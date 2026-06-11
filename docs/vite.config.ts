import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { vocs } from "vocs/vite";

// Used only by `pnpm dev` (which runs Vite directly). `vocs build`/`vocs preview`
// run Vite with `configFile: false` and ignore this file, so the production build
// is unaffected; the plugin list mirrors what the vocs CLI sets up internally.
export default defineConfig({
  plugins: [react(), vocs()],
  optimizeDeps: {
    // mermaid lazy-loads several CJS deps; without pre-bundling, the dev server
    // throws "... does not provide an export named ..." and the diagram fails to
    // render (the production build is unaffected). They are transitive (under
    // mermaid), so use Vite's "owner > dep" form -- a bare name fails to resolve.
    include: [
      "mermaid > dayjs",
      "mermaid > @braintree/sanitize-url",
      "mermaid > dompurify",
      "mermaid > khroma",
      "mermaid > cytoscape",
    ],
  },
});
