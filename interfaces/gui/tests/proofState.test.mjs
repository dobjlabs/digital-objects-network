import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";

const require = createRequire(import.meta.url);
const bundle = await build({
  configFile: false,
  root: fileURLToPath(new URL("..", import.meta.url)),
  logLevel: "silent",
  define: {
    "import.meta.env.VITE_DOBJD_URL": JSON.stringify(""),
    "import.meta.env.DEV": "false",
  },
  build: {
    write: false,
    minify: false,
    lib: {
      entry: fileURLToPath(new URL("../src/shared/state/store.ts", import.meta.url)),
      formats: ["es"],
      fileName: "store",
    },
    rollupOptions: {
      external: ["react"],
      output: { paths: { react: pathToFileURL(require.resolve("react")).href } },
    },
  },
});
const code = (Array.isArray(bundle) ? bundle : [bundle]).flatMap((build) => build.output)
  .find((output) => output.type === "chunk" && output.isEntry).code;

const action = { pluginName: "test", name: "Craft" };
const result = { runId: "run-1", oldRoot: "root", inputFiles: ["in.dobj"], outputFiles: ["out.dobj"] };
const input = { action, inputBindings: [{ objectPath: "in.dobj", label: "Input" }] };
const done = { ...result, phase: "commit", status: "done", message: "Transaction landed", atMs: 1 };
const failed = { runId: "run-1", phase: "commit", status: "failed", message: "expired", atMs: 1 };

function json(value) {
  return new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });
}

test("proof completion is shared by events and polling", async (t) => {
  const descriptors = new Map(["document", "window", "EventSource"].map((key) => [
    key, Object.getOwnPropertyDescriptor(globalThis, key),
  ]));
  t.after(() => {
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  globalThis.document = { querySelector: () => null };
  globalThis.window = { location: { origin: "http://localhost" } };
  let source;
  globalThis.EventSource = class {
    constructor() { source = this; }
    close() { this.closed = true; }
  };
  const { useStore: store } = await import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
  const initial = store.getState();
  const init = () => store.getState().initProofPanel({ runId: "run-1", action, args: ["Input"] });
  t.mock.method(globalThis, "setTimeout", (callback) => { queueMicrotask(callback); return 0; });
  t.mock.method(console, "error", () => {});
  t.mock.method(console, "warn", () => {});

  for (const [status, event] of [["succeeded", done], ["failed", failed]]) {
    await t.test(`${status} has the same terminal state from either follow path`, async (t) => {
      store.setState(initial, true);
      init();
      store.getState().applyRunActionProgress(event);
      const fromEvent = store.getState().proof;
      store.setState({ ...initial, resetProofPanel: () => {} }, true);
      t.mock.method(globalThis, "fetch", async (_url, init) => json(
        init.method === "POST" ? { runId: "run-1" } : { status, result, error: "expired" },
      ));
      await store.getState().runProof(input);
      const fromPoll = store.getState().proof;
      assert.deepEqual(fromPoll, fromEvent);
      assert.equal(fromPoll.runActionId, "run-1");
      if (status === "succeeded") {
        assert.deepEqual(fromPoll.steps.map((step) => step.status), ["done", "done"]);
      }
      assert.equal(source.closed, true);

      for (const late of [event, done, failed, { ...done, phase: "generateProof", status: "running" }]) {
        store.getState().applyRunActionProgress(late);
        assert.equal(store.getState().proof, fromPoll);
      }
    });
  }

  await t.test("polling failures cannot overwrite a success already delivered by events", async (t) => {
    let completed;
    store.setState({ ...initial, resetProofPanel: (runId) => {
      completed = store.getState().proof;
      initial.resetProofPanel(runId);
    } }, true);
    t.mock.method(globalThis, "fetch", async (_url, init) => {
      if (init.method === "POST") return json({ runId: "run-1" });
      source.onmessage({ data: JSON.stringify(done) });
      throw new Error("offline");
    });
    await store.getState().runProof(input);
    assert.equal(completed.status, "summary");
    assert.equal(completed.error, null);
    assert.deepEqual(completed.steps.map((step) => step.status), ["done", "done"]);
    assert.equal(store.getState().proof.status, "idle");
  });

  for (const status of ["succeeded", "failed"]) {
    await t.test(`an old run's ${status} poll cannot change the current run`, async (t) => {
      store.setState(initial, true);
      t.mock.method(globalThis, "fetch", async (_url, init) => {
        if (init.method === "POST") return json({ runId: "run-1" });
        store.getState().resetProofPanel();
        store.getState().initProofPanel({ runId: "run-2", action, args: [] });
        return json({ status, result, error: "expired" });
      });
      await store.getState().runProof(input);
      store.getState().applyRunActionProgress(done);
      assert.equal(store.getState().proof.runActionId, "run-2");
      assert.equal(store.getState().proof.status, "generating");
    });
  }

  await t.test("a failure to start an action is shown without a run id", async (t) => {
    store.setState(initial, true);
    t.mock.method(globalThis, "fetch", async () => { throw new Error("offline"); });
    await store.getState().runProof(input);
    assert.equal(store.getState().proof.runActionId, null);
    assert.equal(store.getState().proof.status, "error");
    assert.equal(store.getState().proof.error, "offline");
    init();
    store.getState().applyRunActionProgress(failed);
    await store.getState().runProof(input);
    assert.equal(store.getState().proof.runActionId, null);
    assert.equal(store.getState().proof.error, "offline");
  });
});
