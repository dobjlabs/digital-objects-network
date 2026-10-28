import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import { build } from "vite";

test("the built index can be served from dobjd's /ui/ mount", async () => {
  const result = await build({
    root: fileURLToPath(new URL("..", import.meta.url)),
    logLevel: "silent",
    build: { write: false },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap((build) => build.output);
  const html = outputs.find((output) => output.fileName === "index.html").source;
  assert.ok(html.includes("<head>"), "dobjd injects its <base> and API origin after a bare <head>");

  const emitted = new Set(outputs.map((output) => output.fileName));
  const references = [...new JSDOM(html).window.document.querySelectorAll("script[src], link[href]")]
    .map((element) => element.getAttribute("src") ?? element.getAttribute("href"));
  assert.ok(references.length > 0, "built index must reference its bundled assets");
  for (const reference of references) {
    assert.ok(reference.startsWith("./"), `${reference} must resolve against the injected <base>`);
    assert.ok(emitted.has(reference.slice(2)), `${reference} is not in the build output`);
  }
});
