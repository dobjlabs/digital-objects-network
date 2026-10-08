import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

async function buildClient(buildUrl, development = false) {
  const result = await build({
    configFile: false,
    root: fileURLToPath(new URL("..", import.meta.url)),
    logLevel: "silent",
    define: {
      "import.meta.env.VITE_DOBJD_URL": JSON.stringify(buildUrl),
      "import.meta.env.DEV": JSON.stringify(development),
    },
    build: {
      write: false,
      minify: false,
      lib: {
        entry: fileURLToPath(new URL("../src/shared/api/httpClient.ts", import.meta.url)),
        formats: ["es"],
        fileName: "httpClient",
      },
    },
  });
  const builds = Array.isArray(result) ? result : [result];
  return builds.flatMap((build) => build.output)
    .find((output) => output.type === "chunk" && output.isEntry).code;
}

test("built client selects the same daemon for HTTP and SSE", async (t) => {
  const customBuild = await buildClient("http://127.0.0.1:7727/");
  const defaultBuild = await buildClient("");
  const developmentBuild = await buildClient("http://127.0.0.1:7727/", true);
  const cases = [
    {
      name: "bundled UI overrides a conflicting build URL on a non-default port",
      code: customBuild,
      meta: "/",
      expected: "http://127.0.0.1:7737",
    },
    {
      name: "standalone UI uses its build URL without a meta tag",
      code: customBuild,
      expected: "http://127.0.0.1:7727",
    },
    {
      name: "standalone meta tag overrides the build URL",
      code: customBuild,
      meta: "http://127.0.0.1:7747/",
      expected: "http://127.0.0.1:7747",
    },
    {
      name: "standalone UI defaults to the daemon port rather than the frontend port",
      code: defaultBuild,
      expected: "http://127.0.0.1:7717",
    },
    {
      name: "development keeps HTTP and SSE on the Vite proxy",
      code: developmentBuild,
      meta: "/",
      expected: "/api",
    },
  ];
  for (const [index, scenario] of cases.entries()) {
    await t.test(scenario.name, async (t) => {
      const descriptors = new Map(["document", "window", "EventSource"].map((key) => [
        key, Object.getOwnPropertyDescriptor(globalThis, key),
      ]));
      t.after(() => {
        for (const [key, descriptor] of descriptors) {
          if (descriptor) Object.defineProperty(globalThis, key, descriptor);
          else delete globalThis[key];
        }
      });
      globalThis.document = {
        querySelector: () => scenario.meta ? { content: scenario.meta } : null,
      };
      globalThis.window = { location: { origin: "http://127.0.0.1:7737" } };
      const eventUrls = [];
      globalThis.EventSource = class {
        constructor(url) { eventUrls.push(url); }
        close() {}
      };
      const fetch = t.mock.method(globalThis, "fetch", async () => new Response("[]"));
      const module = await import(
        `data:text/javascript;base64,${Buffer.from(scenario.code).toString("base64")}#${index}`
      );
      await module.loadObjects();
      const unlistenGlobal = await module.listenRunActionProgress(() => {});
      const unlistenRun = await module.listenRunActionProgressForRun("run-1", () => {});
      assert.equal(fetch.mock.calls[0].arguments[0], `${scenario.expected}/objects`);
      assert.deepEqual(eventUrls, [
        `${scenario.expected}/events`,
        `${scenario.expected}/actions/runs/run-1/events`,
      ]);
      unlistenGlobal();
      unlistenRun();
    });
  }
});
