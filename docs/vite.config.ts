import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import { vocs } from 'vocs/vite'

// Used only by `pnpm dev` (which runs Vite directly). `vocs build`/`vocs preview`
// run Vite with `configFile: false` and ignore this file, so the production build
// is unaffected; the plugin list mirrors what the vocs CLI sets up internally.
export default defineConfig({
  plugins: [react(), vocs()],
  optimizeDeps: {
    // mermaid pulls in dayjs (CJS). Without pre-bundling, the dev server throws
    // "dayjs.min.js does not provide an export named 'default'" and the diagram
    // fails to render. dayjs is a transitive dep (under mermaid), so use Vite's
    // "owner > dep" form -- a bare 'dayjs' fails to resolve from the project root.
    include: ['mermaid > dayjs'],
  },
})
