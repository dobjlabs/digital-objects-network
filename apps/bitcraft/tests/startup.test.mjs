import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { JSDOM, VirtualConsole } from "jsdom";

const html = await readFile(new URL("../ui/index.html", import.meta.url), "utf8");
const qname = (name, pluginName = "bitcraft") => ({ pluginName, name });
const recipe = (name = "CraftPocketknife") => ({
  action: qname(name), totalInputs: [], totalOutputs: [{ class: qname("Pocketknife") }],
  predicateSource: "", mutatedObjects: [],
});
const knife = (status = "live", extra = {}) => ({
  fileName: "pocketknife.dobj", class: qname("Pocketknife"), status,
  fields: { kvs: [] }, ...extra,
});
const state = (runId, status = "queued", extra = {}) => ({
  runId, action: qname("CraftPocketknife"), status, inputObjectPaths: [],
  createdAtMs: 1000, progress: [], error: null, ...extra,
});
const copy = (value) => JSON.parse(JSON.stringify(value));
const flush = async () => { for (let i = 0; i < 5; i++) await new Promise(setImmediate); };

async function page(context, options = {}) {
  const backend = {
    objects: [], actions: [recipe()], states: new Map(), posts: [], discards: [],
    inventoryError: false, listError: false, runError: false, postError: false, syncError: false, ...options,
  };
  const timers = new Map();
  const sources = [];
  const errors = [];
  let nextTimer = 0;
  const virtualConsole = new VirtualConsole();
  virtualConsole.on("jsdomError", (error) => errors.push(error));
  const dom = new JSDOM(html.replace('<meta charset="utf-8">',
    '<meta charset="utf-8"><meta name="dobjd-api-url" content="/">'), {
    url: "http://127.0.0.1:7727/bitcraft/", runScripts: "dangerously", virtualConsole,
    beforeParse(window) {
      window.TextEncoder = TextEncoder;
      window.ResizeObserver = class { observe() {} };
      window.HTMLCanvasElement.prototype.getContext = () => ({ measureText: (text) => ({ width: text.length * 7 }) });
      window.setInterval = (callback, delay) => {
        const id = ++nextTimer;
        timers.set(id, { callback, delay });
        return id;
      };
      window.clearInterval = (id) => timers.delete(id);
      if (options.startupRunId) window.sessionStorage.setItem("bitcraft.startup-pocketknife-run", options.startupRunId);
      window.EventSource = class {
        constructor(url) {
          if (backend.streamUnavailable) throw new Error("SSE unavailable");
          this.url = url; this.closed = false; sources.push(this);
        }
        close() { this.closed = true; }
      };
      window.fetch = async (url, init = {}) => {
        const respond = (value, status = 200) => ({ ok: status < 400, status, json: async () => copy(value) });
        if (url === "class-icons.json") return respond({});
        if (url === "action-icons.json") return respond({ rules: [] });
        if (url === "BITCRAFT_WELCOME.md") return { ok: true, text: async () => "# Welcome" };
        const parsed = new URL(url);
        assert.equal(parsed.origin, "http://127.0.0.1:7727");
        if (parsed.pathname === "/objects/dir") return respond({ path: "/Users/player/.bitcraft/objects" });
        if (parsed.pathname === "/objects/unspent") return respond(backend.objects, backend.inventoryError ? 503 : 200);
        if (parsed.pathname === "/state-root") return respond("0x00", backend.syncError ? 500 : 200);
        const discard = parsed.pathname.match(/^\/objects\/([^/]+)\/discard_unsafe$/);
        if (discard && init.method === "POST") {
          const fileName = decodeURIComponent(discard[1]);
          const object = backend.objects.find((candidate) => candidate.fileName === fileName);
          if (!object) return respond({}, 404);
          backend.discards.push(fileName);
          backend.objects = backend.objects.filter((candidate) => candidate !== object);
          return respond(object);
        }
        if (parsed.pathname === "/settings") return respond({});
        if (parsed.pathname === "/actions") return respond(backend.actions);
        if (parsed.pathname === "/actions/run") {
          const input = JSON.parse(init.body).input;
          backend.posts.push(input);
          if (backend.postError) return respond({}, 503);
          const runId = `run-${backend.posts.length}`;
          backend.states.set(runId, state(runId));
          return respond({ runId, status: "queued", createdAtMs: 1000 });
        }
        if (parsed.pathname === "/actions/runs") {
          return respond([...backend.states.values()].filter((run) => !["succeeded", "failed"].includes(run.status)), backend.listError ? 503 : 200);
        }
        if (parsed.pathname.startsWith("/actions/runs/")) {
          if (backend.daemonDown) throw new TypeError("Failed to fetch");
          const run = backend.states.get(parsed.pathname.split("/").at(-1));
          return respond(run ?? {}, backend.runError ? 503 : run ? 200 : 404);
        }
        throw new Error(`Unexpected request ${url}`);
      };
    },
  });
  context.after(() => { dom.window.close(); assert.deepEqual(errors, []); });
  await flush();
  const { document } = dom.window;
  const tick = async () => {
    for (const [id, timer] of [...timers]) {
      if (timer.delay === 1500 && timers.has(id)) await timer.callback();
    }
    await flush();
  };
  return {
    backend, document, dom, tick, sources,
    splash: document.querySelector("#startup-screen"),
    app: document.querySelector("#inventory-app"),
    jobs: () => [...document.querySelectorAll(".run-row")],
    title: () => document.querySelector("#startup-status").textContent,
    progress: () => document.querySelector("#startup-progress").value,
    async retry() { document.querySelector("#startup-retry").click(); await flush(); },
    async complete(runId = "run-1") {
      backend.states.set(runId, state(runId, "succeeded", { result: { outputFiles: ["pocketknife.dobj"], nullifiedFiles: [] } }));
      backend.objects = [knife()];
      await tick();
    },
  };
}

test("first launch waits for proof, confirmation, and a live knife, then reveals a clean inventory", async (context) => {
  const ui = await page(context);
  assert.equal(ui.backend.posts.length, 1);
  assert.deepEqual(ui.backend.posts[0], { action: qname("CraftPocketknife"), inputObjectPaths: [] });
  assert.equal(ui.app.hidden, true);
  assert.equal(ui.app.inert, true);
  assert.match(ui.title(), /Creating your first pocketknife/);
  assert.equal(ui.progress(), 1);
  assert.equal(ui.document.querySelector("#startup-progress-label").textContent, "Step 2 of 3");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 2 of 3.*Creating your first pocketknife/);
  assert.equal(ui.jobs().length, 0);
  ui.backend.states.set("run-1", state("run-1", "generateProof"));
  await ui.tick();
  assert.equal(ui.progress(), 1, "starting proof generation does not complete a milestone");
  ui.backend.states.set("run-1", state("run-1", "committing"));
  ui.backend.objects = [knife("pending")];
  await ui.tick();
  assert.match(ui.title(), /Waiting for confirmation/);
  assert.equal(ui.document.querySelector("#startup-progress-label").textContent, "Step 3 of 3");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 3 of 3.*Waiting for confirmation/);
  assert.equal(ui.progress(), 2);
  assert.equal(ui.app.hidden, true);
  ui.backend.states.set("run-1", state("run-1", "succeeded"));
  await ui.tick();
  assert.equal(ui.splash.hidden, false, "terminal success alone does not make the knife usable");
  assert.equal(ui.progress(), 2, "confirmation requires a live knife");
  await ui.complete();
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.progress(), 3);
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.app.inert, false);
  assert.equal(ui.jobs().length, 0);
  assert.equal(ui.document.querySelector("#execute-pane").hidden, true);
  assert.equal(ui.document.querySelectorAll(".object-row").length, 2);
  assert.equal(ui.backend.posts.length, 1);
  assert.equal(ui.sources[0].closed, true, "polling handles terminal writes without another SSE event");
});

test("an existing live knife opens inventory immediately, including a held knife", async (context) => {
  const ui = await page(context, { objects: [knife("live", { heldByRunId: "another-job" })] });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.app.hidden, false);
});

for (const status of ["pending", "unknown"]) {
  test(`reopening with a stale ${status} knife and no job discards it and crafts another`, async (context) => {
    const ui = await page(context, { objects: [knife(status)] });
    assert.deepEqual(ui.backend.discards, ["pocketknife.dobj"]);
    assert.equal(ui.backend.posts.length, 1);
    assert.match(ui.title(), /Creating your first pocketknife/);
    assert.equal(ui.progress(), 1, "the discarded knife does not count as a completed proof");
    assert.equal(ui.document.querySelectorAll(".object-row").length, 1, "only the guide is listed");
    await ui.complete();
    assert.equal(ui.app.hidden, false);
    assert.equal(ui.jobs().length, 0);
  });
}

test("a stale knife is kept when the synchronizer cannot be reached", async (context) => {
  const ui = await page(context, { objects: [knife("pending")], syncError: true });
  assert.deepEqual(ui.backend.discards, []);
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /Could not check your pocketknife on the network/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  ui.backend.syncError = false;
  ui.backend.objects = [knife()];
  await ui.retry();
  assert.deepEqual(ui.backend.discards, [], "a knife that landed meanwhile is kept");
  assert.equal(ui.app.hidden, false);
});

for (const status of ["queued", "generateProof", "committing"]) {
  test(`reopening during ${status} resumes the hidden automatic job`, async (context) => {
    const ui = await page(context, {
      states: new Map([["resumed", state("resumed", status)]]), startupRunId: "resumed",
    });
    assert.equal(ui.backend.posts.length, 0);
    assert.equal(ui.jobs().length, 0);
    assert.equal(ui.app.hidden, true);
    assert.equal(ui.progress(), status === "committing" ? 2 : 1);
    await ui.complete("resumed");
    assert.equal(ui.app.hidden, false);
    assert.equal(ui.jobs().length, 0);
  });
}

test("manual pocketknife jobs remain visible", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const list = ui.document.querySelector("#object-list");
  list.dispatchEvent(new ui.dom.window.MouseEvent("contextmenu", { bubbles: true, button: 2 }));
  ui.document.querySelector('.context-menu [role="menuitem"]').click();
  await flush();
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /CraftPocketknife/);
  assert.equal(ui.document.querySelector("#execute-pane").hidden, false);
});

test("a restored manual crafting job is not mistaken for an automatic one", async (context) => {
  const ui = await page(context, { states: new Map([["manual", state("manual")]]) });
  assert.equal(ui.backend.posts.length, 0);
  await ui.complete("manual");
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.jobs().length, 1);
});

test("failed setup reports the cause and retries only on request", async (context) => {
  const ui = await page(context);
  ui.backend.states.set("run-1", state("run-1", "failed", { error: "Network rejected the proof" }));
  await ui.tick();
  assert.match(ui.document.querySelector("#startup-error").textContent, /Network rejected/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.document.querySelector("#startup-open").hidden, true);
  await ui.tick();
  assert.equal(ui.backend.posts.length, 1);
  await ui.retry();
  assert.equal(ui.backend.posts.length, 2);
  assert.equal(ui.jobs().length, 0);
  await ui.complete("run-2");
  assert.equal(ui.app.hidden, false);
});

test("POST failure offers retry but no way into inventory", async (context) => {
  const ui = await page(context, { postError: true });
  assert.match(ui.title(), /could not finish setup/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.document.querySelector("#startup-open").hidden, true);
  assert.match(ui.document.querySelector("#startup-description").textContent, /Try again to make your starter pocketknife/);
  assert.equal(ui.progress(), 1);
  ui.backend.postError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 2);
  await ui.complete("run-2");
  assert.equal(ui.app.hidden, false);
});

test("retry after a lost POST response resumes the accepted job without a duplicate", async (context) => {
  const ui = await page(context, { postError: true });
  ui.backend.states.set("accepted", state("accepted"));
  ui.backend.postError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
  await ui.complete("accepted");
  assert.equal(ui.app.hidden, false);
});

test("active-run lookup failure preserves inventory, prevents blind creation, and can be retried", async (context) => {
  const ui = await page(context, { listError: true, objects: [knife("pending")] });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.document.querySelectorAll(".object-row").length, 2, "opening inventory still offers the loaded objects");
  ui.backend.listError = false;
  ui.backend.objects = [];
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
});

test("inventory lookup failure has a recoverable setup screen", async (context) => {
  const ui = await page(context, { inventoryError: true });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.progress(), 0, "unreachable daemon does not complete preparation");
  ui.backend.inventoryError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
  assert.equal(ui.progress(), 1);
});

for (const status of ["pending", "unknown"]) {
  test(`retry after a failed commit discards the ${status} knife it left and crafts another`, async (context) => {
    const ui = await page(context);
    ui.backend.states.set("run-1", state("run-1", "failed", { error: "Submission failed" }));
    ui.backend.objects = [knife(status)];
    await ui.tick();
    assert.match(ui.document.querySelector("#startup-error").textContent, /Submission failed/);
    assert.deepEqual(ui.backend.discards, [], "nothing is discarded until the user retries");
    assert.equal(ui.backend.posts.length, 1);
    await ui.retry();
    assert.deepEqual(ui.backend.discards, ["pocketknife.dobj"]);
    assert.equal(ui.backend.posts.length, 2);
    assert.match(ui.title(), /Creating your first pocketknife/);
    await ui.complete("run-2");
    assert.equal(ui.app.hidden, false);
  });
}

test("a failed commit retains its completed proof milestone and retry checks progress afresh", async (context) => {
  const ui = await page(context);
  ui.backend.states.set("run-1", state("run-1", "failed", {
    error: "Submission failed",
    progress: [{ phase: "generateProof", status: "done", message: "Proof generation complete" }],
  }));
  ui.backend.objects = [knife("unknown")];
  await ui.tick();
  assert.equal(ui.progress(), 2, "a missed committing event still preserves the completed proof");
  assert.equal(ui.document.querySelector("#startup-progress-label").textContent, "Step 3 of 3");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /could not finish setup/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  await ui.tick();
  assert.equal(ui.progress(), 2);
  await ui.retry();
  assert.equal(ui.backend.posts.length, 2);
  assert.equal(ui.progress(), 1, "the new attempt has not completed its proof");
});

test("a missing recipe gives an explanation and allows opening inventory", async (context) => {
  const ui = await page(context, { actions: [] });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /recipe is unavailable/);
  assert.equal(ui.document.querySelector("#startup-open").hidden, false);
  ui.document.querySelector("#startup-open").click();
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.progress(), 1, "dismissing an error does not complete setup");
  await ui.tick();
  assert.equal(ui.splash.hidden, true, "background refreshes must not reopen dismissed setup");
});

test("polling recovers from unavailable SSE and temporary progress-read errors", async (context) => {
  const ui = await page(context, { streamUnavailable: true });
  assert.equal(ui.backend.posts.length, 1);
  ui.backend.runError = true;
  await ui.tick();
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.match(ui.document.querySelector("#startup-error").textContent, /Could not check progress: .*HTTP 503/);
  ui.backend.runError = false;
  ui.backend.states.set("run-1", state("run-1", "committing"));
  await ui.tick();
  assert.match(ui.title(), /Waiting for confirmation/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, true);
  await ui.complete();
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.backend.posts.length, 1);
});

test("a daemon that stops responding mid-run says how to recover", async (context) => {
  const ui = await page(context);
  ui.backend.daemonDown = true;
  await ui.tick();
  const error = ui.document.querySelector("#startup-error").textContent;
  assert.match(error, /background service stopped responding \(Failed to fetch\)/);
  assert.match(error, /Quit and reopen Bitcraft/);
  assert.match(error, /dobjd\.log in \/Users\/player\/\.bitcraft\./);
  assert.equal(ui.document.querySelector("#startup-open").hidden, true);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
});

test("a different plugin's pocketknife does not suppress startup creation", async (context) => {
  const ui = await page(context, { objects: [knife("live", { class: qname("Pocketknife", "other") })] });
  assert.equal(ui.backend.posts.length, 1);
  assert.equal(ui.app.hidden, true);
});

test("an active mutation that outputs a pocketknife prevents duplicate crafting", async (context) => {
  const ui = await page(context, {
    actions: [recipe(), recipe("GatherWood")],
    states: new Map([["gather", state("gather", "committing", { action: qname("GatherWood") })]]),
  });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.title(), /Waiting for confirmation/);
  await ui.complete("gather");
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.jobs().length, 1);
});
