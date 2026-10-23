import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { JSDOM, VirtualConsole } from "jsdom";

// jsdom does not fetch scripts here, so the page's presentation loader is
// inlined.
const presentationScript = await readFile(new URL("../ui/presentation.js", import.meta.url), "utf8");
const html = (await readFile(new URL("../ui/index.html", import.meta.url), "utf8"))
  .replace('<script src="presentation.js"></script>', () => `<script>${presentationScript}</script>`);
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
    inventoryError: false, listError: false, runError: false, postError: false, syncError: false,
    inventoryReads: 0, runGate: null, ...options,
  };
  const timers = new Map();
  const sources = [];
  const errors = [];
  let nextTimer = 0;
  const virtualConsole = new VirtualConsole();
  virtualConsole.on("jsdomError", (error) => errors.push(error));
  const dom = new JSDOM(html.replace('<meta charset="utf-8">',
    '<meta charset="utf-8"><meta name="dobjd-api-url" content="/">'), {
    url: options.url ?? "http://127.0.0.1:7727/bitcraft/", runScripts: "dangerously", virtualConsole,
    beforeParse(window) {
      window.TextEncoder = TextEncoder;
      window.ResizeObserver = class { observe() {} };
      window.Element.prototype.scrollIntoView = () => {};
      window.HTMLCanvasElement.prototype.getContext = () => ({ measureText: (text) => ({ width: text.length * 7 }) });
      window.setInterval = (callback, delay) => {
        const id = ++nextTimer;
        timers.set(id, { callback, delay });
        return id;
      };
      window.clearInterval = (id) => timers.delete(id);
      if (options.startupRunId) window.sessionStorage.setItem("bitcraft.startup-pocketknife-run", options.startupRunId);
      window.EventSource = class {
        constructor(url) { this.url = url; this.closed = false; sources.push(this); }
        close() { this.closed = true; }
      };
      window.fetch = async (url, init = {}) => {
        const respond = (value, status = 200) => ({ ok: status < 400, status, json: async () => copy(value) });
        if (url === "BITCRAFT_WELCOME.md") return { ok: true, text: async () => "# Welcome" };
        const parsed = new URL(url);
        assert.equal(parsed.origin, "http://127.0.0.1:7727");
        // Presentation packs are files served beside the page; by default
        // there are none.
        if (parsed.pathname.startsWith("/bitcraft/presentation/")) {
          const file = backend.presentation?.[parsed.pathname.slice("/bitcraft/presentation/".length)];
          if (file === undefined) return respond({}, 404);
          if (typeof file === "string") return { ok: true, status: 200, json: async () => JSON.parse(file) };
          return respond(file);
        }
        if (parsed.pathname === "/objects/dir") return respond({ path: "/Users/player/.bitcraft/objects" });
        if (parsed.pathname === "/objects/unspent") {
          backend.inventoryReads++;
          if (backend.inventoryDown) throw new TypeError("Failed to fetch");
          return respond(backend.objects, backend.inventoryError ? 503 : 200);
        }
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
        if (parsed.pathname === "/settings") return respond(backend.settings ?? {});
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
          // Read before waiting, so a held read answers with the state it saw.
          const run = backend.states.get(decodeURIComponent(parsed.pathname.split("/").at(-1)));
          const runError = backend.runError;
          if (backend.runGate) await backend.runGate;
          return respond(run ?? {}, runError ? 503 : run ? 200 : 404);
        }
        throw new Error(`Unexpected request ${url}`);
      };
    },
  });
  context.after(() => { dom.window.close(); assert.deepEqual(errors, []); });
  await flush();
  const { document } = dom.window;
  // Fires the startup screen's inventory poll.
  const tick = async () => {
    for (const [id, timer] of [...timers]) {
      if (timer.delay === 1500 && timers.has(id)) await timer.callback();
    }
    await flush();
  };
  const openStreams = (runId) => sources.filter((source) => !source.closed
    && source.url.endsWith(`/actions/runs/${encodeURIComponent(runId)}/events`));
  // A progress event on the run's stream, with no change to its state.
  const emit = async (runId) => {
    for (const source of openStreams(runId)) source.onmessage?.({ data: "{}" });
    await flush();
  };
  // The stream erroring, as when its connection drops or dobjd closes it.
  const dropStream = async (runId) => {
    for (const source of openStreams(runId)) source.onerror?.({});
    await flush();
  };
  // Mirrors dobjd: a state change is recorded with a progress event, and the
  // stream closes once the run has finished.
  const publish = async (runId, status, extra = {}) => {
    const previous = backend.states.get(runId) ?? state(runId);
    const progress = extra.progress ?? [...previous.progress, {
      phase: status === "generateProof" ? "generateProof" : "commit",
      status: status === "failed" ? "failed" : "running",
      message: status,
    }];
    backend.states.set(runId, { ...previous, status, ...extra, progress });
    const streams = openStreams(runId);
    for (const source of streams) source.onmessage?.({ data: JSON.stringify(progress.at(-1)) });
    if (status === "succeeded" || status === "failed") {
      for (const source of streams) source.onerror?.({});
    }
    await flush();
  };
  return {
    backend, document, dom, tick, sources, timers, emit, dropStream, publish,
    splash: document.querySelector("#startup-screen"),
    app: document.querySelector("#inventory-app"),
    jobs: () => [...document.querySelectorAll(".run-row")],
    title: () => document.querySelector("#startup-status").textContent,
    progress: () => document.querySelector("#startup-progress").value,
    async retry() { document.querySelector("#startup-retry").click(); await flush(); },
    async complete(runId = "run-1") {
      backend.objects = [knife()];
      await publish(runId, "succeeded", { result: { outputFiles: ["pocketknife.dobj"], nullifiedFiles: [] } });
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
  assert.equal(ui.document.querySelector("#status-bar").closest("[hidden], [inert]"), null,
    "the status bar shows under the setup screen");
  assert.match(ui.title(), /Creating your first pocketknife/);
  assert.equal(ui.progress(), 1);
  assert.equal(ui.document.querySelector("#startup-progress-label").textContent, "Step 2 of 3");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 2 of 3.*Creating your first pocketknife/);
  assert.equal(ui.jobs().length, 0);
  await ui.publish("run-1", "generateProof");
  assert.equal(ui.progress(), 1, "starting proof generation does not complete a milestone");
  ui.backend.objects = [knife("pending")];
  await ui.publish("run-1", "committing");
  assert.match(ui.title(), /Waiting for confirmation/);
  assert.equal(ui.document.querySelector("#startup-progress-label").textContent, "Step 3 of 3");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 3 of 3.*Waiting for confirmation/);
  assert.equal(ui.progress(), 2);
  assert.equal(ui.app.hidden, true);
  await ui.publish("run-1", "succeeded");
  assert.equal(ui.sources[0].closed, true, "the stream closing delivers the terminal state");
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
  assert.equal(ui.document.querySelectorAll(".object-row.flashing").length, 0,
    "the starter knife does not flash when the inventory opens");
  assert.equal(ui.backend.posts.length, 1);
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

test("objects that change after setup still flash", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const list = ui.document.querySelector("#object-list");
  list.dispatchEvent(new ui.dom.window.MouseEvent("contextmenu", { bubbles: true, button: 2 }));
  ui.document.querySelector('.context-menu [role="menuitem"]').click();
  await flush();
  ui.backend.objects = [knife(), knife("pending", { fileName: "second-knife.dobj" })];
  await ui.publish("run-1", "committing");
  assert.deepEqual(
    [...ui.document.querySelectorAll(".object-row.flashing")].map((row) => row.title),
    ["second-knife.dobj"],
  );
});

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
  await ui.publish("run-1", "failed", { error: "Network rejected the proof" });
  assert.match(ui.document.querySelector("#startup-error").textContent, /Network rejected/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
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
  assert.deepEqual(
    [...ui.document.querySelectorAll("#startup-actions button")].map((button) => button.textContent),
    ["Try again", "Copy error"],
  );
  assert.match(ui.document.querySelector("#startup-description").textContent, /opens once you have a starter pocketknife/);
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

test("active-run lookup failure prevents blind creation and can be retried", async (context) => {
  const ui = await page(context, { listError: true, objects: [knife("pending")] });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.app.hidden, true);
  ui.backend.listError = false;
  ui.backend.objects = [];
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
});

test("inventory lookup failure has a recoverable setup screen", async (context) => {
  const ui = await page(context, { inventoryError: true });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.progress(), 0, "a failed inventory read does not complete preparation");
  assert.equal(ui.document.querySelector("#startup-error").textContent, "/objects/unspent: HTTP 503",
    "an HTTP error is shown as it is, since dobjd answered");
  ui.backend.inventoryError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
  assert.equal(ui.progress(), 1);
});

for (const status of ["pending", "unknown"]) {
  test(`retry after a failed commit discards the ${status} knife it left and crafts another`, async (context) => {
    const ui = await page(context);
    ui.backend.objects = [knife(status)];
    await ui.publish("run-1", "failed", { error: "Submission failed" });
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
  ui.backend.objects = [knife("unknown")];
  await ui.publish("run-1", "failed", {
    error: "Submission failed",
    progress: [{ phase: "generateProof", status: "done", message: "Proof generation complete" }],
  });
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

test("a missing recipe gives an explanation and retry picks up the installed recipe", async (context) => {
  const ui = await page(context, { actions: [] });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /recipe is unavailable/);
  assert.equal(ui.app.hidden, true);
  assert.equal(ui.progress(), 1);
  ui.backend.actions = [recipe()];
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
  await ui.complete();
  assert.equal(ui.app.hidden, false);
});

test("a failed progress read clears on the next stream event", async (context) => {
  const ui = await page(context);
  assert.equal(ui.backend.posts.length, 1);
  ui.backend.runError = true;
  await ui.emit("run-1");
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.match(ui.document.querySelector("#startup-error").textContent, /Could not check progress: .*HTTP 503/);
  ui.backend.runError = false;
  await ui.publish("run-1", "committing");
  assert.match(ui.title(), /Waiting for confirmation/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, true);
  await ui.complete();
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.backend.posts.length, 1);
});

test("a daemon that stops responding mid-run says how to recover", async (context) => {
  const ui = await page(context);
  ui.backend.daemonDown = true;
  await ui.dropStream("run-1");
  const error = ui.document.querySelector("#startup-error").textContent;
  assert.match(error, /background service isn't responding \(Failed to fetch\)/);
  assert.match(error, /Quit and reopen Bitcraft/);
  assert.match(error, /dobjd\.log in \/Users\/player\/\.bitcraft\./);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
});

test("progress reads reload the inventory only when the run's state changed", async (context) => {
  const ui = await page(context);
  const before = ui.backend.inventoryReads;
  await ui.emit("run-1");
  await ui.emit("run-1");
  assert.equal(ui.backend.inventoryReads, before, "unchanged state leaves the inventory alone");
  await ui.publish("run-1", "generateProof");
  assert.equal(ui.backend.inventoryReads, before + 1);
});

test("a stream close that arrives during a read is served after it", async (context) => {
  const ui = await page(context);
  let releaseRead;
  ui.backend.runGate = new Promise((resolve) => { releaseRead = resolve; });
  await ui.emit("run-1");
  ui.backend.objects = [knife()];
  await ui.publish("run-1", "succeeded");
  ui.backend.runGate = null;
  releaseRead();
  await flush();
  await ui.tick();
  assert.equal(ui.app.hidden, false, "the queued read sees the terminal state");
});

test("a daemon that is not running on first load says how to recover", async (context) => {
  const ui = await page(context, { inventoryDown: true });
  assert.equal(ui.backend.posts.length, 0);
  const error = ui.document.querySelector("#startup-error").textContent;
  assert.match(error, /^Bitcraft's background service isn't responding \(Failed to fetch\)\. Quit and reopen Bitcraft to restart it\.$/,
    "the log location is left out when the objects directory is unknown");
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  assert.equal(ui.document.querySelector("#status-bar").closest("[hidden], [inert]"), null);
  ui.backend.inventoryDown = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
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

test("the top bar keeps the files view selected and opens Recipe Info in one window", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const { document } = ui;
  assert.equal(document.querySelector("#tab-bar"), null);
  assert.equal(document.querySelector("#objects-view").hidden, false);
  assert.equal(document.querySelector("#recipebook-view").hidden, true);
  const files = document.querySelector("#top-bar .toolbar-button.selected");
  assert.equal(files.tagName, "SPAN");
  assert.equal(files.getAttribute("aria-label"), "Files");

  const opened = [];
  let focused = 0;
  const referenceWindow = { closed: false, focus: () => { focused++; } };
  ui.dom.window.open = (url, name, features) => {
    opened.push({ url: String(url), name, features });
    return referenceWindow;
  };
  const book = document.querySelector("#reference-button");
  book.click();
  assert.deepEqual(opened.map(({ url, name }) => ({ url, name })), [
    { url: "http://127.0.0.1:7727/bitcraft/?page=recipebook", name: "bitcraft-recipe-info" },
  ]);
  book.click();
  assert.equal(opened.length, 1);
  assert.equal(focused, 1);
  referenceWindow.closed = true;
  book.click();
  assert.equal(opened.length, 2);
});

test("the Recipe Info window shows only recipes and runs no startup", async (context) => {
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&action=bitcraft::CraftPocketknife",
    actions: [recipe(), recipe("GatherWood"), recipe("Other", "another-plugin")].map((action, index) =>
      index === 2 ? { ...action, action: qname("Other", "another-plugin") } : action),
  });
  const { document } = ui;
  assert.equal(document.body.classList.contains("reference-window"), true);
  assert.equal(document.title, "Bitcraft reference");
  assert.equal(document.querySelector("#recipebook-view").hidden, false);
  assert.deepEqual(ui.backend.posts, []);
  assert.equal(ui.backend.inventoryReads, 0);
  assert.deepEqual([...document.querySelectorAll(".recipe-row .action-name")].map((name) => name.textContent),
    ["bitcraft::CraftPocketknife", "bitcraft::GatherWood"]);
  assert.equal(document.querySelector("#recipe-details-body .details-title").textContent, "bitcraft::CraftPocketknife");
  document.querySelectorAll(".recipe-row")[1].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.equal(new URL(ui.dom.window.location.href).searchParams.get("action"), "bitcraft::GatherWood");
});

test("the summary icon opens the Inventory Summary in one window", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const opened = [];
  ui.dom.window.open = (url, name) => {
    opened.push({ url: String(url), name });
    return { closed: false, focus() {} };
  };
  const button = ui.document.querySelector("#summary-button");
  button.click();
  button.click();
  assert.deepEqual(opened, [
    { url: "http://127.0.0.1:7727/bitcraft/?page=summary", name: "bitcraft-inventory-summary" },
  ]);
});

test("the Inventory Summary counts live objects and refreshes on job events", async (context) => {
  const item = (fileName, name, status = "live", pluginName = "bitcraft") => ({
    fileName, class: qname(name, pluginName), status, fields: { kvs: [] },
  });
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=summary",
    objects: [
      knife(), item("wood-1.dobj", "Wood"), item("wood-2.dobj", "Wood"),
      item("stick.dobj", "Stick", "pending"), item("log.dobj", "Log", "live", "craft-basics"),
    ],
  });
  const { document } = ui;
  const text = (selector) => document.querySelector(selector).textContent;
  const fields = (section) => [...document.querySelectorAll(`[data-section="${section}"] dt`)]
    .map((term) => `${term.textContent.trim()} ${term.nextElementSibling.textContent}`);
  assert.equal(document.title, "Bitcraft Inventory Summary");
  assert.equal(document.body.classList.contains("secondary-window"), true);
  assert.deepEqual(ui.backend.posts, []);
  assert.equal(text(".summary-total"), "2 bits");
  assert.equal(text(".summary-subtitle"), "4 live objects in total");
  assert.deepEqual(fields("general"), ["Total Bit Value: 2", "Total objects: 4"]);
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1", "bitcraft::Wood: 2"]);

  const polls = () => [...ui.timers.values()].filter((timer) => timer.delay === 10000);
  assert.equal(polls().length, 1, "polls while the stick is pending");

  document.querySelector('[data-section="general"]').open = false;
  ui.backend.objects.push(item("wood-3.dobj", "Wood"));
  const events = ui.sources.find((source) => source.url.endsWith("/events"));
  events.onmessage({ data: "{}" });
  await flush();
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1", "bitcraft::Wood: 3"]);
  assert.equal(document.querySelector('[data-section="general"]').open, false);

  const reads = ui.backend.inventoryReads;
  events.onopen();
  await flush();
  assert.equal(ui.backend.inventoryReads, reads, "the first connection does not read again");
  events.onopen();
  await flush();
  assert.equal(ui.backend.inventoryReads, reads + 1, "a reconnect reads the inventory");

  ui.backend.objects.find((object) => object.fileName === "stick.dobj").status = "live";
  await polls()[0].callback();
  await flush();
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1", "bitcraft::Stick: 1", "bitcraft::Wood: 3"]);
  assert.equal(polls().length, 0, "stops polling once everything is live");
});

test("the right-hand buttons open the site and Settings; Discord does nothing yet", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const opened = [];
  ui.dom.window.open = (url, name) => {
    opened.push({ url: String(url), name });
    return { closed: false, focus() {} };
  };
  ui.document.querySelector("#discord-button").click();
  assert.deepEqual(opened, []);
  ui.document.querySelector("#about-button").click();
  ui.document.querySelector("#settings-button").click();
  assert.deepEqual(opened, [
    { url: "https://bitcraft.digiobjects.net/", name: "_blank" },
    { url: "http://127.0.0.1:7727/bitcraft/?page=settings", name: "bitcraft-settings" },
  ]);
});

test("the Settings window shows the connection addresses read-only and runs no startup", async (context) => {
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=settings",
    settings: { synchronizerApiUrl: "http://127.0.0.1:3000", relayerApiUrl: "http://127.0.0.1:3200", mcpEnabled: false },
  });
  const { document } = ui;
  const field = (id) => document.querySelector(`#settings-${id}`);
  assert.equal(document.title, "Bitcraft Settings");
  assert.equal(document.querySelector("#settings-view").hidden, false);
  assert.deepEqual(ui.backend.posts, []);
  assert.equal(ui.backend.inventoryReads, 0);
  assert.equal(field("synchronizer").value, "http://127.0.0.1:3000");
  assert.equal(field("relayer").value, "http://127.0.0.1:3200");
  assert.equal(field("synchronizer").readOnly, true);
  assert.equal(field("relayer").readOnly, true);
});

test("the main window's title carries the Total Bit Value of live objects", async (context) => {
  const wood = (fileName, status = "live") => ({ fileName, class: qname("Wood"), status, fields: { kvs: [] } });
  const ui = await page(context, { objects: [knife(), wood("wood-1.dobj"), wood("wood-2.dobj"), wood("wood-3.dobj", "pending")] });
  assert.equal(ui.document.title, "Bitcraft (Total Bit Value: 2)");
  assert.equal(ui.document.querySelector("#objects-footer"), null);
  ui.backend.objects.find((object) => object.fileName === "wood-3.dobj").status = "live";
  await ui.dom.window.reloadObjects();
  await flush();
  assert.equal(ui.document.title, "Bitcraft (Total Bit Value: 3)");
});

const bitcraftPack = {
  index: { packs: ["bitcraft/pack.json"] },
  "bitcraft/pack.json": {
    format: 1,
    plugin: "bitcraft",
    classes: {
      Pocketknife: { icon: "assets/classes/pocketknife.png", summary: "Your first tool.", description: "Lasts forever." },
    },
    actions: {
      CraftPocketknife: { icon: "assets/actions/CraftPocketknife.svg", summary: "Make a knife.", description: "From nothing." },
    },
  },
};
const presentationFiles = (pack) => ({ "index.json": pack.index, "bitcraft/pack.json": pack["bitcraft/pack.json"] });

test("presentation packs give classes and actions their icons and text", async (context) => {
  const ui = await page(context, { objects: [knife()], presentation: presentationFiles(bitcraftPack) });
  const { document } = ui;
  const rowIcon = document.querySelector('.object-row[title="pocketknife.dobj"] .icon-tile img');
  assert.equal(rowIcon.src, "http://127.0.0.1:7727/bitcraft/presentation/bitcraft/assets/classes/pocketknife.png");
  document.querySelector('.object-row[title="pocketknife.dobj"]').dispatchEvent(
    new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  const details = document.querySelector("#details-body");
  assert.equal(details.querySelector(".description-summary").textContent, "Your first tool.");
  assert.equal(details.querySelector(".description-text").textContent, "Lasts forever.");
  assert.equal(details.querySelector(".large-class-icon").src, rowIcon.src);
});

test("Recipe Info shows each action's pack icon and text", async (context) => {
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&action=bitcraft::CraftPocketknife",
    presentation: presentationFiles(bitcraftPack),
  });
  const { document } = ui;
  assert.equal(document.querySelector(".recipe-row img.action-icon").src,
    "http://127.0.0.1:7727/bitcraft/presentation/bitcraft/assets/actions/CraftPocketknife.svg");
  const details = document.querySelector("#recipe-details-body");
  assert.equal(details.querySelector("img.action-icon").width, 128);
  assert.equal(details.querySelector(".description-summary").textContent, "Make a knife.");
  assert.equal(details.querySelector(".description-text").textContent, "From nothing.");
});

for (const [label, files] of [
  ["no index", {}],
  ["an index naming a missing pack", { "index.json": { packs: ["bitcraft/pack.json"] } }],
  ["a malformed pack", { "index.json": { packs: ["bitcraft/pack.json"] }, "bitcraft/pack.json": "{ not json" }],
  ["a pack with malformed entries", { "index.json": { packs: [7, "bitcraft/pack.json"] },
    "bitcraft/pack.json": { plugin: "bitcraft", classes: { Pocketknife: { icon: 5, summary: [] } }, actions: null } }],
]) {
  test(`${label} leaves plain icons and no descriptions`, async (context) => {
    const ui = await page(context, { objects: [knife()], presentation: files });
    const { document } = ui;
    assert.equal(ui.app.hidden, false);
    const row = document.querySelector('.object-row[title="pocketknife.dobj"]');
    assert.equal(row.querySelector(".icon-tile"), null);
    assert.notEqual(row.querySelector("svg"), null);
    row.dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
    assert.equal(document.querySelector("#details-body .description-summary"), null);
  });
}

test("a manifest description is the fallback, except the placeholder", async (context) => {
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&action=bitcraft::GatherWood",
    actions: [{ ...recipe(), description: "-" }, { ...recipe("GatherWood"), description: "Gather some wood." }],
  });
  const { document } = ui;
  assert.equal(document.querySelector("#recipe-details-body .description-text").textContent, "Gather some wood.");
  document.querySelectorAll(".recipe-row")[0].dispatchEvent(
    new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.equal(document.querySelector("#recipe-details-body .description-text"), null);
});
