import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import { vocs } from 'vocs/vite'

// The vocs CLI (`vocs dev`/`vocs build`) runs Vite with `configFile: false`, so
// it ignores this file. We run Vite directly (see package.json scripts) to get
// `optimizeDeps` honored; the plugin list mirrors what the vocs CLI sets up.
export default defineConfig({
  plugins: [react(), vocs()],
  optimizeDeps: {
    // mermaid pulls in dayjs (CJS). Without pre-bundling, the dev server throws
    // "dayjs.min.js does not provide an export named 'default'" and the diagram
    // fails to render. Pre-bundling rewrites it to a proper ESM default export.
    include: ['dayjs'],
  },
})
