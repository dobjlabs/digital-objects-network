import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";

const require = createRequire(import.meta.url);

export async function buildModule(entry, { define = {}, external = [] } = {}) {
  const result = await build({
    configFile: false,
    root: fileURLToPath(new URL("..", import.meta.url)),
    logLevel: "silent",
    define,
    build: {
      write: false,
      minify: false,
      lib: { entry: fileURLToPath(entry), formats: ["es"], fileName: "test" },
      rollupOptions: {
        external,
        output: {
          paths: Object.fromEntries(external.map((name) => [
            name, pathToFileURL(require.resolve(name)).href,
          ])),
        },
      },
    },
  });
  return (Array.isArray(result) ? result : [result]).flatMap((build) => build.output)
    .find((output) => output.type === "chunk" && output.isEntry).code;
}

export function importModule(code, instance = "") {
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}#${instance}`);
}
