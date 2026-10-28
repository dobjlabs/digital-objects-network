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
    objects: [], transactions: [], proofJobs: new Map(), submissions: [], actions: [recipe()], states: new Map(), posts: [], discards: [],
    inventoryError: false, listError: false, runError: false, postError: false, syncError: false,
    inventoryReads: 0, runReads: 0, runGate: null, ...options,
  };
  const timers = new Map();
  const holds = new Map();
  const sources = [];
  const errors = [];
  let nextTimer = 0;
  const virtualConsole = new VirtualConsole();
  virtualConsole.on("jsdomError", (error) => errors.push(error));
  const dom = new JSDOM(html.replace('<meta charset="utf-8">',
    '<meta charset="utf-8"><meta name="dobjd-api-url" content="/">'), {
    url: options.url ?? "http://127.0.0.1:7727/bitcraft/", runScripts: "dangerously", virtualConsole,
    beforeParse(window) {
      window.bitcraftDesktop = options.desktop ?? false;
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
      // Startup steps stay up for a minimum time. Holds end at the next turn
      // of the event loop unless `holdSteps` keeps them until `elapse`.
      window.setTimeout = (callback, delay) => {
        const id = ++nextTimer;
        holds.set(id, { callback, delay });
        if (!options.holdSteps) {
          setImmediate(() => {
            if (holds.delete(id)) callback();
          });
        }
        return id;
      };
      window.clearTimeout = (id) => holds.delete(id);
      if (options.startupRunId) window.sessionStorage.setItem("bitcraft.startup-pocketknife-run", options.startupRunId);
      for (const [key, value] of Object.entries(options.sessionStorage ?? {})) {
        window.sessionStorage.setItem(key, value);
      }
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
          const snapshot = copy(backend.objects);
          if (backend.inventoryGate) await backend.inventoryGate;
          return respond(snapshot, backend.inventoryError ? 503 : 200);
        }
        if (parsed.pathname === "/transactions") return respond(backend.transactions, backend.transactionError ? 503 : 200);
        if (parsed.pathname === "/proof-jobs") {
          const snapshot = copy([...backend.proofJobs.values()]);
          if (backend.activityGate) await backend.activityGate;
          return respond(snapshot, backend.proofJobError ? 503 : 200);
        }
        const transactionPath = parsed.pathname.match(/^\/transactions\/([^/]+)$/);
        if (transactionPath && !init.method) {
          const record = backend.transactions.find((record) => record.txFinal === decodeURIComponent(transactionPath[1]));
          return respond(record ?? { error: "Unknown transaction" }, record ? 200 : 404);
        }
        if (parsed.pathname === "/transactions/submit") {
          backend.submissions.push(JSON.parse(init.body).txFinals);
          if (backend.submitError) return respond({}, 503);
          const records = JSON.parse(init.body).txFinals.map((txFinal) => {
            const record = backend.transactions.find((record) => record.txFinal === txFinal);
            record.state = "submitted";
            return record;
          });
          return respond(records);
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
        const raw = parsed.pathname.match(/^\/objects\/([^/]+)\/raw$/);
        if (raw) {
          const object = backend.objects.find((candidate) => candidate.fileName === decodeURIComponent(raw[1]));
          return { ok: Boolean(object), status: object ? 200 : 404, text: async () => JSON.stringify(object) };
        }
        if (parsed.pathname === "/actions") return respond(backend.actions);
        if (parsed.pathname === "/classes") return respond(backend.classes ?? []);
        if (parsed.pathname === "/runs" && init.method === "POST") {
          const input = JSON.parse(init.body).input;
          backend.posts.push(input);
          if (backend.postError) return respond({}, 503);
          const runId = `run-${backend.posts.length}`;
          backend.states.set(runId, state(runId, "queued", { action: input.action, inputObjectPaths: input.inputObjectPaths }));
          if (backend.postGate) await backend.postGate;
          return respond({ runId, status: "queued", createdAtMs: 1000 });
        }
        if (parsed.pathname === "/runs") {
          const snapshot = copy([...backend.states.values()]
            .filter((run) => parsed.searchParams.get("status") !== "active" || !["succeeded", "failed"].includes(run.status)));
          if (backend.activityGate) await backend.activityGate;
          return respond(snapshot, backend.listError ? 503 : 200);
        }
        if (parsed.pathname.startsWith("/runs/")) {
          backend.runReads++;
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
  // Ends every startup step's minimum time.
  const elapse = async () => {
    for (const [id, hold] of [...holds]) {
      if (holds.delete(id)) hold.callback();
    }
    await flush();
  };
  const openStreams = (runId) => sources.filter((source) => !source.closed
    && source.url.endsWith(`/runs/${encodeURIComponent(runId)}/events`));
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
  let nextSeq = 1;
  let publishedObjects = copy(backend.objects);
  const event = async (payload, seq = nextSeq++) => {
    if (payload.type === "proof-job") {
      const { type, ...job } = payload;
      backend.proofJobs.set(job.jobId, copy(job));
    }
    if (payload.type === "object") {
      backend.objects = backend.objects.filter((object) => object.fileName !== payload.fileName);
      const { type, ...object } = payload;
      if (object.status !== "nullified") backend.objects.push(copy(object));
    } else if (payload.type === "object-removed") {
      backend.objects = backend.objects.filter((object) => object.fileName !== payload.fileName);
    } else if (payload.type === "transaction") {
      backend.transactions = backend.transactions.filter((record) => record.txFinal !== payload.txFinal);
      const { type, ...record } = payload;
      backend.transactions.push(copy(record));
    } else if (["transaction-deleted", "transaction-pruned"].includes(payload.type)) {
      backend.transactions = backend.transactions.filter((record) => record.txFinal !== payload.txFinal);
    }
    for (const source of sources.filter((source) => !source.closed && source.url === "http://127.0.0.1:7727/events")) {
      source.onmessage?.({ data: JSON.stringify({ ...payload, seq }) });
    }
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
    const objects = copy(backend.objects);
    for (const object of objects) {
      if (JSON.stringify(object) !== JSON.stringify(publishedObjects.find((previous) => previous.fileName === object.fileName))) {
        await event({ type: "object", ...object });
      }
    }
    for (const previous of publishedObjects) {
      if (!objects.some((object) => object.fileName === previous.fileName)) await event({ type: "object-removed", fileName: previous.fileName });
    }
    publishedObjects = objects;
    const streams = openStreams(runId);
    for (const source of streams) source.onmessage?.({ data: JSON.stringify(progress.at(-1)) });
    if (status === "succeeded" || status === "failed") {
      for (const source of streams) source.onerror?.({});
    }
    await flush();
  };
  return {
    backend, document, dom, sources, timers, holds, elapse, emit, dropStream, publish, event,
    splash: document.querySelector("#startup-screen"),
    app: document.querySelector("#inventory-app"),
    jobs: () => [...document.querySelectorAll(".run-row")],
    title: () => document.querySelector("#startup-status").textContent,
    progress: () => document.querySelector("#startup-progress").value,
    async retry() { document.querySelector("#startup-retry").click(); await flush(); },
    async complete(runId = "run-1") {
      backend.objects = [knife()];
      await publish(runId, "succeeded", { result: { outputFiles: ["pocketknife.dobj"], inputFiles: [] } });
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
  assert.match(ui.title(), /Loading cryptography engine/);
  assert.equal(ui.progress(), 2);
  assert.equal(ui.document.querySelector("#startup-progress-label").hidden, true, "no step number while setup works");
  assert.equal(ui.document.querySelector("#startup-description").hidden, true, "no description while setup works");
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 3 of 4.*Loading cryptography engine/);
  assert.equal(ui.jobs().length, 0);
  await ui.publish("run-1", "generateProof");
  assert.equal(ui.progress(), 2, "starting proof generation does not complete a milestone");
  ui.backend.objects = [knife("pending")];
  await ui.publish("run-1", "committing");
  assert.match(ui.title(), /Connecting to Bitcraft network/);
  assert.equal(ui.document.querySelector("#startup-progress-label").hidden, true);
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /Step 4 of 4.*Connecting to Bitcraft network/);
  assert.equal(ui.progress(), 3);
  assert.equal(ui.app.hidden, true);
  await ui.publish("run-1", "succeeded");
  assert.equal(ui.sources.find((source) => source.url.includes("/runs/")).closed, true, "the stream closing delivers the terminal state");
  assert.equal(ui.splash.hidden, false, "terminal success alone does not make the knife usable");
  assert.equal(ui.progress(), 3, "confirmation requires a live knife");
  await ui.complete();
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.progress(), 4);
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.app.inert, false);
  assert.equal(ui.jobs().length, 0);
  assert.equal(ui.document.querySelector("#execute-pane").hidden, true);
  assert.equal(ui.document.querySelectorAll(".object-row").length, 2);
  assert.equal(ui.document.querySelectorAll(".object-row.flashing").length, 0,
    "the starter knife does not flash when the inventory opens");
  assert.equal(ui.backend.posts.length, 1);
});

test("each startup step stays up for its minimum time, even when the work behind it is done", async (context) => {
  const ui = await page(context, { holdSteps: true });
  assert.equal(ui.backend.posts.length, 1, "the work goes ahead while a step is held");
  assert.match(ui.title(), /Loading inventory/);
  assert.equal(ui.progress(), 1);
  assert.equal(ui.holds.size, 1);
  assert.equal([...ui.holds.values()][0].delay, 1000);
  await ui.elapse();
  assert.match(ui.title(), /Loading cryptography engine/);
  assert.equal(ui.progress(), 2);
  ui.backend.objects = [knife("pending")];
  await ui.publish("run-1", "committing");
  assert.match(ui.title(), /Loading cryptography engine/, "confirmation waits for the proof step's minimum");
  await ui.elapse();
  assert.match(ui.title(), /Connecting to Bitcraft network/);
  assert.equal(ui.progress(), 3);
  await ui.complete();
  assert.equal(ui.splash.hidden, false, "the inventory waits for the confirmation step's minimum");
  assert.equal(ui.app.hidden, true);
  await ui.elapse();
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.progress(), 4);
});

test("a held startup step skips steps the startup did not reach", async (context) => {
  const ui = await page(context, { holdSteps: true, objects: [knife()] });
  assert.match(ui.title(), /Loading inventory/);
  assert.equal(ui.app.hidden, true, "an existing knife still shows the first step for its minimum");
  await ui.elapse();
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.holds.size, 0);
});

test("errors and retries replace a held startup step at once", async (context) => {
  const ui = await page(context, { holdSteps: true, inventoryError: true });
  assert.match(ui.title(), /could not finish setup/, "an error on the step on display is not held back");
  assert.equal(ui.progress(), 1);
  ui.backend.inventoryError = false;
  await ui.retry();
  assert.match(ui.title(), /Loading inventory/);
  assert.equal(ui.holds.size, 1, "retry restarts the first step's minimum");
  await ui.elapse();
  assert.match(ui.title(), /Loading cryptography engine/);
  await ui.publish("run-1", "failed", { error: "Proof failed" });
  assert.match(ui.document.querySelector("#startup-error").textContent, /Proof failed/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  await ui.retry();
  assert.match(ui.title(), /Loading inventory/, "retry goes back to the first step without waiting");
  assert.equal(ui.progress(), 1);
});

test("every startup step shows the Bitcraft logo", async (context) => {
  const ui = await page(context);
  const logos = () => [...ui.document.querySelectorAll("#startup-screen img")].map((image) => image.src);
  const logo = ["http://127.0.0.1:7727/bitcraft/assets/bitcraft-logo.png"];
  assert.match(ui.title(), /Loading cryptography engine/);
  assert.deepEqual(logos(), logo);
  await ui.publish("run-1", "failed", { error: "Proof failed" });
  assert.match(ui.title(), /could not finish setup/);
  assert.deepEqual(logos(), logo);
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
    assert.match(ui.title(), /Loading cryptography engine/);
    assert.equal(ui.progress(), 2, "the discarded knife does not count as a completed proof");
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
  assert.match(ui.document.querySelector("#startup-error").textContent, /Could not reach the Bitcraft network/);
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
    assert.equal(ui.progress(), status === "committing" ? 3 : 2);
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
    [...ui.document.querySelectorAll(".object-row.flashing")].map((row) => row.dataset.fileName),
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
  await flush();
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
  assert.match(ui.document.querySelector("#startup-description").textContent, /opens once setup finishes/);
  assert.equal(ui.progress(), 2);
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
  assert.equal(ui.progress(), 1, "a failed inventory read does not complete preparation");
  assert.equal(ui.document.querySelector("#startup-error").textContent, "/objects/unspent: HTTP 503",
    "an HTTP error is shown as it is, since dobjd answered");
  ui.backend.inventoryError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
  assert.equal(ui.progress(), 2);
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
    assert.match(ui.title(), /Loading cryptography engine/);
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
  assert.equal(ui.progress(), 3, "a missed committing event still preserves the completed proof");
  const label = ui.document.querySelector("#startup-progress-label");
  assert.equal(label.hidden, false, "an error says where setup stopped");
  assert.equal(label.textContent, "Stopped at step 4 of 4");
  assert.equal(ui.document.querySelector("#startup-description").hidden, false);
  assert.match(ui.document.querySelector("#startup-progress").getAttribute("aria-valuetext"), /could not finish setup/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  await flush();
  assert.equal(ui.progress(), 3);
  await ui.retry();
  assert.equal(ui.backend.posts.length, 2);
  assert.equal(ui.progress(), 2, "the new attempt has not completed its proof");
});

test("a missing recipe gives an explanation and retry picks up the installed recipe", async (context) => {
  const ui = await page(context, { actions: [] });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /A recipe Bitcraft needs to start is unavailable/);
  assert.equal(ui.app.hidden, true);
  assert.equal(ui.progress(), 2);
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
  assert.match(ui.title(), /Connecting to Bitcraft network/);
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

test("run progress does not re-read the inventory", async (context) => {
  const ui = await page(context);
  const before = ui.backend.inventoryReads;
  await ui.emit("run-1");
  await ui.emit("run-1");
  assert.equal(ui.backend.inventoryReads, before, "unchanged state leaves the inventory alone");
  await ui.publish("run-1", "generateProof");
  assert.equal(ui.backend.inventoryReads, before);
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
  await flush();
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
  assert.match(ui.title(), /Connecting to Bitcraft network/);
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

test("desktop agent buttons request the selected app without starting game actions", async (context) => {
  const ui = await page(context, { objects: [knife()], desktop: true });
  const opened = [];
  ui.dom.window.open = (url, name, features) => opened.push({ url, name, features });
  for (const agent of ["claude", "codex"]) {
    const button = ui.document.getElementById(`${agent}-button`);
    assert.equal(button.hidden, false);
    assert.match(button.getAttribute("aria-label"), new RegExp(agent, "i"));
    assert.equal(button.dataset.tooltip, `Automate with ${agent === "claude" ? "Claude" : "Codex"}`);
    button.querySelector("svg").dispatchEvent(new ui.dom.window.MouseEvent("click", { bubbles: true }));
    button.click();
  }
  assert.deepEqual(opened.map(({ url }) => url), [
    "bitcraft://agent/claude", "bitcraft://agent/claude",
    "bitcraft://agent/codex", "bitcraft://agent/codex",
  ]);
  assert.equal(ui.document.getElementById("agent-divider").hidden, false);
  assert.equal(ui.backend.posts.length, 0);
});

test("browser-only pages hide desktop agent controls", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  for (const id of ["claude-button", "codex-button", "agent-divider"]) {
    assert.equal(ui.document.getElementById(id).hidden, true);
  }
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

test("Recipe Info lists classes, then actions, and shows a class's details and source", async (context) => {
  const wood = {
    class: qname("Wood"), hash: "0xwood", description: "Logs.", liveCount: 0,
    producedBy: [qname("GatherWood"), qname("Missing")], consumedBy: [qname("CraftPocketknife")],
    predicateSource: "IsWood(state, chain) = OR(\n  IsWoodFromGatherWood(state, chain)\n)",
  };
  const other = { ...wood, class: qname("Log", "another-plugin"), producedBy: [], consumedBy: [] };
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&class=bitcraft::Wood",
    classes: [wood, other],
    actions: [recipe(), { ...recipe("GatherWood"), predicateSource: "GatherWood(io, private: wood) = AND(\n  Equal(io, wood)\n)" }],
  });
  const { document } = ui;
  assert.deepEqual([...document.querySelectorAll("#recipe-list > li")]
    .map((row) => (row.querySelector(".row-text > div") ?? row).textContent),
  ["Classes", "bitcraft::Wood", "Actions", "bitcraft::CraftPocketknife", "bitcraft::GatherWood"],
  "another plugin's class is left out");
  assert.equal(document.querySelector(".recipe-row.selected .reference-summary").textContent, "Logs.");
  const details = document.querySelector("#recipe-details-body");
  assert.equal(details.querySelector(".details-title").textContent, "bitcraft::Wood");
  assert.equal(details.querySelector(".details-meta").textContent, "object class");
  assert.equal(details.querySelector(".description-text").textContent, "Logs.");
  assert.deepEqual([...details.querySelectorAll(".reference-link")].map((link) => link.textContent),
    ["bitcraft::GatherWood", "bitcraft::Missing", "bitcraft::CraftPocketknife"]);
  const source = [...details.querySelectorAll("details.collapsible-section")]
    .find((section) => section.querySelector("summary").textContent === "Class Source");
  assert.equal(source.open, false);
  assert.equal(source.querySelector("pre").textContent, [
    "IsWood(",
    "  state,",
    "  chain",
    ") = OR(",
    "  IsWoodFromGatherWood(state, chain)",
    ")",
    "",
    "// bitcraft::GatherWood",
    "GatherWood(",
    "  io,",
    "private:",
    "  wood",
    ") = AND(",
    "  Equal(io, wood)",
    ")",
    "",
    "// bitcraft::Missing",
    "// Source not available.",
  ].join("\n"));

  details.querySelector(".reference-link").click();
  const address = new URL(ui.dom.window.location.href).searchParams;
  assert.equal(address.get("action"), "bitcraft::GatherWood");
  assert.equal(address.get("class"), null);
  assert.equal(document.querySelector("#recipe-details-body .details-title").textContent, "bitcraft::GatherWood");
  assert.equal(document.querySelector(".recipe-row.selected .action-name").textContent, "bitcraft::GatherWood");
});

test("class and action names in the main window's details open them in Recipe Info", async (context) => {
  const ui = await page(context, {
    objects: [knife(), { ...knife(), fileName: "second.dobj" }],
    states: new Map([["job-1", state("job-1", "generateProof", { action: qname("GatherWood") })]]),
  });
  const { document } = ui;
  const press = (element, init = {}) =>
    element.dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true, ...init }));
  const opened = [];
  let referenceWindow = null;
  ui.dom.window.open = (url, name) => {
    opened.push({ url: String(url), name });
    referenceWindow = {
      closed: false, focused: 0, shown: [], canShow: true, location: { href: String(url) },
      focus() { this.focused++; },
      showReferenceEntry(entry) { this.shown.push(entry); return this.canShow; },
    };
    return referenceWindow;
  };
  const reference = "http://127.0.0.1:7727/bitcraft/?page=recipebook";

  press(document.querySelector('.object-row[data-file-name="pocketknife.dobj"]'));
  const classLink = document.querySelector("#details-body .class-link");
  assert.equal(classLink.textContent, "bitcraft::Pocketknife");
  classLink.click();
  assert.deepEqual(opened, [{ url: `${reference}&class=bitcraft%3A%3APocketknife`, name: "bitcraft-recipe-info" }]);

  press(document.querySelector('.object-row[data-file-name="second.dobj"]'), { ctrlKey: true });
  assert.equal(document.querySelectorAll("#details-body .class-link").length, 2, "each selected object's class links");
  document.querySelectorAll("#details-body .class-link")[1].click();
  assert.equal(opened.length, 1, "an open Recipe Info window is reused");
  assert.deepEqual(copy(referenceWindow.shown), [{ kind: "class", name: "bitcraft::Pocketknife" }]);
  assert.equal(referenceWindow.focused, 1);

  press(ui.jobs()[0]);
  const actionLink = document.querySelector("#details-body .action-link");
  assert.equal(actionLink.textContent, "bitcraft::GatherWood");
  referenceWindow.canShow = false;
  actionLink.click();
  assert.equal(opened.length, 1);
  assert.equal(referenceWindow.location.href, `${reference}&action=bitcraft%3A%3AGatherWood`,
    "a window that cannot select the entry yet is sent to its address");
  assert.equal(referenceWindow.focused, 2);
});

test("Recipe Info links an action's classes to their details", async (context) => {
  const pocketknife = {
    class: qname("Pocketknife"), hash: "0xknife", description: "-", liveCount: 0,
    producedBy: [qname("CraftPocketknife")], consumedBy: [], predicateSource: "",
  };
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&action=bitcraft::CraftPocketknife",
    classes: [pocketknife],
  });
  const { document } = ui;
  const classLink = document.querySelector("#recipe-details-body .class-link");
  assert.equal(classLink.textContent, "bitcraft::Pocketknife");
  classLink.click();
  assert.equal(new URL(ui.dom.window.location.href).searchParams.get("class"), "bitcraft::Pocketknife");
  assert.equal(document.querySelector("#recipe-details-body .details-title").textContent, "bitcraft::Pocketknife");
  assert.equal(document.querySelector(".recipe-row.selected").textContent.startsWith("bitcraft::Pocketknife"), true);
  document.querySelector("#recipe-details-body .action-link").click();
  assert.equal(document.querySelector("#recipe-details-body .details-title").textContent, "bitcraft::CraftPocketknife");
});

test("Recipe Info lists summaries and shows where each class and action is defined", async (context) => {
  const pexe = "/Users/player/.bitcraft/actions/bitcraft.pexe";
  const wood = {
    class: qname("Wood"), hash: "0xwood", description: "-", liveCount: 0,
    producedBy: [], consumedBy: [], predicateSource: "",
  };
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook&action=bitcraft::CraftPocketknife",
    classes: [wood],
    actions: [{ ...recipe(), sourcePath: pexe, description: "Make a knife." }, { ...recipe("GatherWood"), sourcePath: pexe }],
  });
  const { document } = ui;
  assert.deepEqual([...document.querySelectorAll(".reference-summary")].map((line) => line.textContent),
    ["object class", "Make a knife.", "executable action"]);
  const definedIn = () => [...document.querySelectorAll("#recipe-details-body .field-table th")]
    .find((header) => header.textContent === "defined in").nextElementSibling.textContent;
  assert.equal(definedIn(), pexe);
  document.querySelectorAll(".recipe-row")[0].dispatchEvent(
    new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.equal(document.querySelector("#recipe-details-body .details-title").textContent, "bitcraft::Wood");
  assert.equal(definedIn(), pexe, "a class shows its plugin's file");
});

test("an object's details leave out its hashes and say when it has no properties", async (context) => {
  const axe = {
    fileName: "axe.dobj", class: qname("WoodAxe"), status: "live", contentHash: "0xcontent", classHash: "0xclass",
    fields: { kvs: [["key", { Raw: "secret" }], ["durability", { Int: "60" }]] },
  };
  const ui = await page(context, { objects: [{ ...knife(), contentHash: "0xcontent", classHash: "0xclass" }, axe] });
  const { document } = ui;
  const select = (fileName) => document.querySelector(`.object-row[data-file-name="${fileName}"]`)
    .dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  const labels = () => [...document.querySelectorAll("#details-body .field-table th")].map((header) => header.textContent);

  select("pocketknife.dobj");
  assert.deepEqual(labels(), ["stable_identifier", "key"]);
  assert.equal(document.querySelector("#details-body .empty-note").textContent, "No properties to show.");

  select("axe.dobj");
  assert.deepEqual(labels(), ["stable_identifier", "key", "durability"]);
  assert.equal(document.querySelector("#details-body .empty-note"), null);
});

test("View raw opens a window titled with the file's name that shows its stored text", async (context) => {
  const main = await page(context, { objects: [knife()] });
  main.document.querySelector('.object-row[data-file-name="pocketknife.dobj"]')
    .dispatchEvent(new main.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  const link = [...main.document.querySelectorAll("#details-body a")].find((anchor) => anchor.textContent === "View raw");
  assert.equal(link.href, "http://127.0.0.1:7727/bitcraft/?page=raw&file=pocketknife.dobj");
  const opened = [];
  main.dom.window.open = (url, name, features) => {
    opened.push({ url: String(url), name, features });
    return null;
  };
  link.click();
  // The test page measures text at 7px a character: "pocketknife.dobj" is
  // 112px, plus 90px each side.
  assert.deepEqual(opened, [{ url: link.href, name: "_blank", features: "noopener,popup,width=480,height=420" }],
    "a short name gets the minimum width");
  main.document.querySelector('.object-row[data-file-name="pocketknife.dobj"]')
    .dispatchEvent(new main.dom.window.MouseEvent("dblclick", { bubbles: true }));
  assert.equal(opened[1].features, "noopener,popup,width=480,height=420");
  const longName = "bitcraft__miningdrill_e618b29d73604017_d1d8bf5829530d65.dobj";
  main.backend.objects.push({ ...knife(), fileName: longName });
  main.dom.window.dispatchEvent(new main.dom.window.Event("focus"));
  await flush();
  main.document.querySelector(`.object-row[data-file-name="${longName}"]`)
    .dispatchEvent(new main.dom.window.MouseEvent("dblclick", { bubbles: true }));
  assert.equal(opened[2].features, `noopener,popup,width=${longName.length * 7 + 180},height=420`,
    "a long name widens the window to fit it");

  const ui = await page(context, { url: link.href, objects: [knife()] });
  const { document } = ui;
  assert.equal(document.title, "pocketknife.dobj");
  assert.equal(document.body.classList.contains("secondary-window"), true);
  assert.equal(document.querySelector("#raw-view").hidden, false);
  assert.deepEqual(JSON.parse(document.querySelector("#raw-text").textContent), knife());
  assert.deepEqual(ui.backend.posts, [], "the window runs none of the main window's startup");

  const missing = await page(context, { url: "http://127.0.0.1:7727/bitcraft/?page=raw&file=gone.dobj" });
  assert.equal(missing.document.querySelector("#raw-text").textContent, "Could not read gone.dobj: HTTP 404");
});

test("resting on an object row shows its BIT value after a quarter second", async (context) => {
  const axe = {
    fileName: "axe.dobj", class: qname("WoodAxe"), status: "live",
    fields: { kvs: [["durability", { Int: "60" }]] },
  };
  const ui = await page(context, { objects: [knife(), axe], holdSteps: true });
  await ui.elapse();
  const { document } = ui;
  const tooltip = document.querySelector("#row-tooltip");
  const hover = (fileName, type = "mouseenter") => document.querySelector(`.object-row[data-file-name="${fileName}"]`)
    .dispatchEvent(new ui.dom.window.MouseEvent(type, { clientX: 40, clientY: 60 }));
  assert.equal(document.querySelector(".object-row").title, "", "no native tooltip competes with it");

  hover("axe.dobj");
  assert.equal(tooltip.hidden, true, "not at once");
  assert.deepEqual([...ui.holds.values()].map((hold) => hold.delay), [250]);
  hover("axe.dobj", "mousemove");
  await ui.elapse();
  assert.equal(tooltip.hidden, false);
  assert.equal(tooltip.textContent, "1.20 BIT", "60% of a new wood axe's 2");
  assert.equal(tooltip.style.left, "52px");
  assert.equal(tooltip.style.top, "78px");

  hover("axe.dobj", "mouseleave");
  assert.equal(tooltip.hidden, true);
  hover("axe.dobj");
  hover("axe.dobj", "mouseleave");
  await ui.elapse();
  assert.equal(tooltip.hidden, true, "leaving before the delay cancels it");
  hover("BITCRAFT_WELCOME.md");
  assert.equal(tooltip.hidden, true, "the welcome guide has no value");
});

test("Recipe Info lists the classes in game order, then any others by name", async (context) => {
  const names = ["Bit", "BucketOfIron", "BucketOfStone", "BucketOfWood", "Gem", "Iron", "IronAxe",
    "MiningDrill", "Ore", "Pocketknife", "Quarry", "Stick", "Stone", "StoneAxe", "Wood", "WoodAxe"];
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=recipebook",
    classes: names.map((name) => ({
      class: qname(name), hash: "0x", description: "-", liveCount: 0,
      producedBy: [], consumedBy: [], predicateSource: "",
    })),
  });
  const listed = [...ui.document.querySelectorAll("#recipe-list > li")]
    .map((row) => (row.querySelector(".row-text > div") ?? row).textContent);
  assert.deepEqual(listed.slice(0, listed.indexOf("Actions")), [
    "Classes",
    "bitcraft::Pocketknife", "bitcraft::Wood", "bitcraft::Stone", "bitcraft::Iron", "bitcraft::Bit",
    "bitcraft::Stick", "bitcraft::WoodAxe", "bitcraft::StoneAxe", "bitcraft::IronAxe",
    "bitcraft::Quarry", "bitcraft::MiningDrill",
    "bitcraft::BucketOfWood", "bitcraft::BucketOfStone", "bitcraft::BucketOfIron",
    "bitcraft::Gem", "bitcraft::Ore",
  ]);
});

test("the welcome guide's name is semibold and object names are not", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const weight = (fileName) => ui.dom.window.getComputedStyle(
    ui.document.querySelector(`.object-row[data-file-name="${fileName}"] .object-name`),
  ).fontWeight;
  assert.equal(weight("BITCRAFT_WELCOME.md"), "600");
  assert.notEqual(weight("pocketknife.dobj"), "600");
});

const quarry = (level) => ({
  fileName: "quarry.dobj", class: qname("Quarry"), status: "live",
  fields: { kvs: [["level", { Int: String(level) }]] },
});
const valueSamples = (...samples) => ({
  "bitcraft.inventory-value-samples": JSON.stringify(samples.map(([ago, value]) => ({ atMs: Date.now() - ago, value }))),
});

test("BIT/sec shows how fast the inventory's value grew over the last 5 minutes", async (context) => {
  // A level 3 quarry is worth 280; the inventory was worth 10 BIT 10 minutes
  // ago, so over the 5-minute window it grew 270 BIT in 300 seconds.
  const ui = await page(context, {
    objects: [knife(), quarry(3)], sessionStorage: valueSamples([600000, 10]),
  });
  const rate = () => ui.document.querySelector("#inventory-rate").textContent;
  assert.equal(rate(), "0.90 BIT/sec");
  assert.equal(ui.document.title, "Bitcraft (280.00 BIT - 0.90 BIT/sec)");
  const ticks = [...ui.timers.values()].filter((timer) => timer.delay === 1000);
  assert.ok(ticks.length > 0, "the rate is refreshed every second");

  const stored = JSON.parse(ui.dom.window.sessionStorage.getItem("bitcraft.inventory-value-samples"));
  assert.deepEqual(stored.map((sample) => sample.value), [10, 280], "the samples are kept for a reload");
});

test("BIT/sec counts only the window, and can fall", async (context) => {
  // 190 BIT 8 minutes ago is the value the window starts from; 400 a minute
  // ago, 280 now: 90 BIT more over 300 seconds.
  const ui = await page(context, {
    objects: [knife(), quarry(3)], sessionStorage: valueSamples([900000, 0], [480000, 190], [60000, 400]),
  });
  assert.equal(ui.document.querySelector("#inventory-rate").textContent, "0.30 BIT/sec");
  const fell = await page(context, {
    objects: [knife(), quarry(3)], sessionStorage: valueSamples([600000, 430]),
  });
  assert.equal(fell.document.querySelector("#inventory-rate").textContent, "-0.50 BIT/sec");
});

test("BIT/sec always averages over 5 minutes, and a 0.00 rate is not shown", async (context) => {
  const ui = await page(context, { objects: [knife(), quarry(3)] });
  const rateShown = (page) => !page.document.querySelector("#inventory-rate").hidden
    && !page.document.querySelector("#inventory-rate-divider").hidden;
  assert.equal(rateShown(ui), false, "with one sample nothing has grown, and a 0.00 rate is not shown");
  assert.equal(ui.document.title, "Bitcraft (280.00 BIT)");
  // 250 BIT ten seconds ago, 280 now: 30 BIT more, spread over 300 seconds
  // rather than 10.
  const recent = await page(context, {
    objects: [knife(), quarry(3)], sessionStorage: valueSamples([10000, 250]),
  });
  assert.equal(recent.document.querySelector("#inventory-rate").textContent, "0.10 BIT/sec");
  assert.equal(rateShown(recent), true);
  assert.equal(recent.document.title, "Bitcraft (280.00 BIT - 0.10 BIT/sec)");
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

test("the Inventory Summary applies object events and reconciles untracked outputs", async (context) => {
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
  // A row's label, then its values separated by " | ".
  const fields = (section) => [...document.querySelectorAll(`[data-section="${section}"] dt`)]
    .map((term) => {
      const values = [];
      for (let cell = term.nextElementSibling; cell?.tagName === "DD"; cell = cell.nextElementSibling) {
        values.push(cell.textContent);
      }
      return `${term.textContent.trim()} ${values.join(" | ")}`;
    });
  assert.equal(document.title, "Bitcraft Inventory Summary");
  assert.equal(document.body.classList.contains("secondary-window"), true);
  assert.deepEqual(ui.backend.posts, []);
  assert.equal(text(".summary-total"), "2.00 BIT");
  assert.equal(document.querySelector(".summary-subtitle"), null, "no object total under the title");
  assert.equal(document.querySelector('[data-section="general"]'), null, "no General section");
  const objectsTitle = () => document.querySelector('[data-section="classes"] summary').textContent;
  assert.equal(objectsTitle(), "Objects (3):", "this plugin's live objects, as listed");
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1 | 0.00 BIT", "bitcraft::Wood: 2 | 2.00 BIT"]);

  const polls = () => [...ui.timers.values()].filter((timer) => timer.delay === 10000);
  assert.equal(polls().length, 1, "polls while the stick is pending");

  document.querySelector('[data-section="classes"]').open = false;
  const events = ui.sources.find((source) => source.url.endsWith("/events"));
  const eventReads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...item("wood-3.dobj", "Wood") });
  assert.equal(ui.backend.inventoryReads, eventReads, "an object event needs no inventory request");
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1 | 0.00 BIT", "bitcraft::Wood: 3 | 3.00 BIT"]);
  assert.equal(objectsTitle(), "Objects (4):");
  assert.equal(document.querySelector('[data-section="classes"]').open, false, "a collapsed section stays collapsed");

  const reads = ui.backend.inventoryReads;
  events.onopen();
  await flush();
  assert.equal(ui.backend.inventoryReads, reads + 1, "the first connection covers changes after the initial snapshot");
  events.onopen();
  await flush();
  assert.equal(ui.backend.inventoryReads, reads + 2, "a reconnect reads the inventory");

  ui.backend.objects.find((object) => object.fileName === "stick.dobj").status = "live";
  await polls()[0].callback();
  await flush();
  assert.deepEqual(fields("classes"), [
    "bitcraft::Pocketknife: 1 | 0.00 BIT", "bitcraft::Stick: 1 | 0.50 BIT", "bitcraft::Wood: 3 | 3.00 BIT",
  ]);
  assert.equal(polls().length, 0, "stops polling once everything is live");
});

test("the gear opens Settings, and the top bar has no site or Discord links", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const opened = [];
  ui.dom.window.open = (url, name) => {
    opened.push({ url: String(url), name });
    return { closed: false, focus() {} };
  };
  assert.equal(ui.document.querySelector("#about-button"), null);
  assert.equal(ui.document.querySelector("#discord-button"), null);
  ui.document.querySelector("#settings-button").click();
  assert.deepEqual(opened, [
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
  assert.equal(field("version").textContent, "Alpha 1.0");
  assert.equal(document.querySelector("#settings-view img"), null, "no logo");
  for (const [id, url, text] of [
    ["website", "https://bitcraft.sh/", "https://bitcraft.sh"],
    ["discord", "https://discord.gg/cWtZZYrer", "Discord"],
  ]) {
    assert.equal(field(id).href, url);
    assert.equal(field(id).textContent.trim(), text);
    assert.equal(field(id).target, "_blank", `${id} opens outside the Settings window`);
  }
});

test("the top bar and the window title show the Inventory Value of live objects", async (context) => {
  const wood = (fileName, status = "live") => ({ fileName, class: qname("Wood"), status, fields: { kvs: [] } });
  const ui = await page(context, { objects: [knife(), wood("wood-1.dobj"), wood("wood-2.dobj"), wood("wood-3.dobj", "pending")] });
  const value = () => ui.document.querySelector("#inventory-value-amount").textContent;
  assert.equal(value(), "2.00 BIT");
  assert.equal(ui.document.querySelector("#inventory-value").textContent.replace(/\s+/g, " ").trim(),
    "Total Inventory Value: 2.00 BIT");
  assert.equal(ui.document.title, "Bitcraft (2.00 BIT)");
  const opened = [];
  ui.dom.window.open = (url, name) => {
    opened.push({ url: String(url), name });
    return { closed: false, focus() {} };
  };
  ui.document.querySelector("#inventory-value").click();
  assert.deepEqual(opened, [{ url: "http://127.0.0.1:7727/bitcraft/?page=summary", name: "bitcraft-inventory-summary" }],
    "the value opens the Inventory Summary");
  assert.equal(ui.document.querySelector("#objects-footer"), null);
  ui.backend.objects.find((object) => object.fileName === "wood-3.dobj").status = "live";
  ui.dom.window.dispatchEvent(new ui.dom.window.Event("focus"));
  await flush();
  assert.equal(value(), "3.00 BIT");
  assert.equal(ui.document.title, "Bitcraft (3.00 BIT)");
});

test("a worn tool is worth the fraction of its durability it has left", async (context) => {
  const axe = (fileName, name, durability) => ({
    fileName, class: qname(name), status: "live",
    fields: { kvs: [["durability", { Int: String(durability) }]] },
  });
  const ui = await page(context, {
    objects: [knife(), axe("wood-axe.dobj", "WoodAxe", 60), axe("stone-axe.dobj", "StoneAxe", 0), axe("iron-axe.dobj", "IronAxe", 100)],
  });
  // 60% of a wood axe's 2, nothing for the spent stone axe, all of an iron axe's 9.
  assert.equal(ui.document.querySelector("#inventory-value-amount").textContent, "10.20 BIT");
});

test("the top bar's icons have tooltips", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  assert.deepEqual([...ui.document.querySelectorAll("#top-bar .toolbar-button:not([hidden])")].map((button) => button.dataset.tooltip),
    ["Objects", "Stats", "Reference", "Settings"]);
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
  const rowIcon = document.querySelector('.object-row[data-file-name="pocketknife.dobj"] .icon-tile img');
  assert.equal(rowIcon.src, "http://127.0.0.1:7727/bitcraft/presentation/bitcraft/assets/classes/pocketknife.png");
  document.querySelector('.object-row[data-file-name="pocketknife.dobj"]').dispatchEvent(
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
    const row = document.querySelector('.object-row[data-file-name="pocketknife.dobj"]');
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

test("the context menu offers only the level-up the tool's level and buckets allow", async (context) => {
  const levelUp = (level) => ({
    action: qname(`LevelUpQuarry${level}`), predicateSource: "", mutatedObjects: [{ inputIndex: 0, outputIndex: 0 }],
    totalInputs: ["Quarry", "BucketOfWood", "BucketOfStone"].map((name) => ({ class: qname(name) })),
    totalOutputs: [{ class: qname("Quarry") }],
  });
  const item = (fileName, name, fields) => ({
    fileName, class: qname(name), status: "live",
    fields: { kvs: Object.entries(fields).map(([key, value]) => [key, { Int: String(value) }]) },
  });
  const objects = [knife(), item("quarry.dobj", "Quarry", { level: 2 }),
    item("wood.dobj", "BucketOfWood", { n: 80 }), item("stone.dobj", "BucketOfStone", { n: 40 })];
  const ui = await page(context, { objects, actions: [recipe(), 1, 2, 3, 4, 5].map((level) =>
    typeof level === "number" ? levelUp(level) : level) });
  const { document, dom } = ui;
  const menuFor = () => {
    const rows = ["quarry.dobj", "wood.dobj", "stone.dobj"]
      .map((fileName) => document.querySelector(`.object-row[data-file-name="${fileName}"]`));
    rows[0].dispatchEvent(new dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
    for (const row of rows.slice(1)) {
      row.dispatchEvent(new dom.window.MouseEvent("mousedown", { button: 0, ctrlKey: true, bubbles: true }));
    }
    rows[0].dispatchEvent(new dom.window.MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    const labels = [...document.querySelectorAll(".context-menu .menu-item-label")].map((label) => label.textContent);
    dom.window.closeContextMenu();
    return labels;
  };
  assert.deepEqual(menuFor(), ["Open all", "bitcraft::LevelUpQuarry2"]);
  ui.backend.objects[2] = item("wood.dobj", "BucketOfWood", { n: 79 });
  dom.window.dispatchEvent(new dom.window.Event("focus"));
  await flush();
  assert.deepEqual(menuFor(), ["Open all", "No available actions"]);
});

const wood = (fileName, extra = {}) => knife("live", { fileName, class: qname("Wood"), ...extra });
const transaction = (state = "submitted", extra = {}) => ({
  txFinal: "0xabc", state, owner: "lost-run", action: qname("CraftPocketknife"),
  inputFiles: [], outputFiles: ["pocketknife.dobj"], ...extra,
});
const inventoryStream = (ui) => ui.sources.find((source) => source.url === "http://127.0.0.1:7727/events");
const proofJob = (jobId, status = "proving", extra = {}) => ({
  jobId, action: qname("GatherWood"), inputObjectPaths: [], status,
  createdAtMs: 1000, startedAtMs: 1001, message: status, ...extra,
});

test("external runs appear live, including a run already finished when its first event is read", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  ui.backend.states.set("external", state("external", "generateProof", { action: qname("GatherWood") }));
  await ui.event({ type: "run-action-progress", runId: "external", phase: "generateProof", status: "running" });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /GatherWood/);
  const streams = () => ui.sources.filter((source) => !source.closed && source.url.endsWith("/runs/external/events"));
  assert.equal(streams().length, 1);
  await ui.event({ type: "run-action-progress", runId: "external", phase: "generateProof", status: "running" });
  assert.equal(ui.jobs().length, 1);
  assert.equal(streams().length, 1);
  ui.backend.states.set("fast", state("fast", "succeeded", {
    action: qname("GatherWood"), result: { inputFiles: [], outputFiles: [] }, finishedAtMs: 1100,
  }));
  await ui.event({ type: "run-action-progress", runId: "fast", phase: "commit", status: "done" });
  assert.equal(ui.jobs().length, 2);
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  assert.equal(ui.sources.some((source) => source.url.endsWith("/runs/fast/events")), false);
});

test("concurrent first events share one run lookup", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  ui.backend.states.set("external", state("external", "generateProof"));
  let release;
  ui.backend.runGate = new Promise((resolve) => { release = resolve; });
  const reads = ui.backend.runReads;
  for (let i = 0; i < 5; i++) {
    await ui.event({ type: "run-action-progress", runId: "external", phase: "generateProof", status: "running" });
  }
  assert.equal(ui.backend.runReads, reads + 1);
  ui.backend.runGate = null;
  release();
  await flush();
  assert.equal(ui.jobs().length, 1);
});

test("separate proving, recording and submission remain one selected job through confirmation", async (context) => {
  const ui = await page(context, { objects: [knife(), wood("wood.dobj")] });
  const proof = (status, extra) => proofJob("proof-1", status, { inputObjectPaths: ["wood.dobj"], ...extra });
  const record = (status, extra) => transaction(status, {
    owner: "proof-1", action: qname("GatherWood"), inputFiles: ["wood.dobj"], outputFiles: ["output.dobj"],
    recordedAtMs: 2000, ...extra,
  });
  await ui.event({ type: "proof-job", ...proof("proving") });
  assert.equal(ui.jobs().length, 1);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.equal(ui.dom.window.inputsAvailable(["wood.dobj"]), false);
  await ui.event({ type: "transaction", ...record("recorded") });
  await ui.event({ type: "proof-job", ...proof("done", { txFinal: "0xabc", finishedAtMs: 2001 }) });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /Ready to submit/);
  assert.equal(ui.jobs()[0].classList.contains("selected"), true);
  const details = () => ui.document.querySelector("#details-body").textContent;
  assert.match(details(), /proof-1/);
  assert.match(details(), /0xabc/);
  assert.match(details(), /objects\/output\.dobj/);
  await ui.event({ type: "transaction", ...record("submitted", { submitAttemptedAtMs: 3000, submittedAtMs: 3001 }) });
  assert.equal(ui.jobs().length, 1);
  assert.match(details(), /submitted/);
  assert.equal(ui.jobs()[0].classList.contains("in-progress"), true);
  await ui.event({ type: "transaction", ...record("landed", { settledAtMs: 4000, groundingRoot: "root", txHash: "eth-hash" }) });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  assert.match(details(), /eth-hash/);
  assert.match(details(), /\.nullified\/wood\.dobj/);
  assert.equal(ui.jobs()[0].classList.contains("in-progress"), false);
  await ui.event({ type: "proof-job", ...proof("done", { txFinal: "0xabc", finishedAtMs: 2001 }) });
  await ui.event({ type: "transaction", ...record("recorded") });
  assert.match(ui.jobs()[0].textContent, /succeeded/, "late proof or record snapshots cannot undo settlement");
});

test("transaction-first activity coalesces when its proof job arrives", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "proof-1" }) });
  assert.equal(ui.jobs().length, 1);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "done", {
    action: qname("CraftPocketknife"), txFinal: "0xabc", finishedAtMs: 2000,
  }) });
  assert.equal(ui.jobs().length, 1);
  assert.equal(ui.jobs()[0].classList.contains("selected"), true);
  assert.match(ui.document.querySelector("#details-body").textContent, /proof-1/);
});

test("a run and proof job linked to the same transaction merge without losing selection", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "proof-job", ...proofJob("proof-1") });
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  ui.backend.states.set("external", state("external", "committing", { action: qname("GatherWood") }));
  await ui.event({ type: "run-action-progress", runId: "external", phase: "commit", status: "running" });
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "external", action: qname("GatherWood") }) });
  assert.equal(ui.jobs().length, 2, "there is no shared identity until the proof reports its transaction");
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "done", { txFinal: "0xabc" }) });
  assert.equal(ui.jobs().length, 1);
  assert.equal(ui.jobs()[0].classList.contains("selected"), true);
  assert.equal(ui.sources.filter((source) => !source.closed && source.url.endsWith("/runs/external/events")).length, 1,
    "the merged job keeps following its run");
  ui.backend.states.set("external", state("external", "committing", {
    action: qname("GatherWood"), progress: [{ atMs: 2000, phase: "commit", status: "running", message: "New run progress" }],
  }));
  await ui.emit("external");
  assert.match(ui.document.querySelector(".run-log tr").textContent, /New run progress/);
  await ui.event({ type: "transaction", ...transaction("landed", {
    owner: "external", action: qname("GatherWood"), settledAtMs: 3000,
  }) });
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  assert.match(ui.document.querySelector(".run-log tr").textContent, /Transaction landed/);
  assert.equal(ui.sources.filter((source) => !source.closed && source.url.endsWith("/runs/external/events")).length, 0,
    "settlement closes the follower even before the terminal run snapshot arrives");
});

test("a local POST response merges activity discovered before it returned", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  let release;
  ui.backend.postGate = new Promise((resolve) => { release = resolve; });
  const started = ui.dom.window.startRun(recipe(), []);
  await flush();
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  await ui.event({ type: "run-action-progress", runId: "run-1", phase: "generateProof", status: "running" });
  ui.backend.postGate = null;
  release();
  await started;
  await flush();
  assert.equal(ui.jobs().length, 1);
  assert.equal(ui.jobs()[0].classList.contains("selected"), true);
  assert.equal(ui.sources.filter((source) => !source.closed && source.url.endsWith("/runs/run-1/events")).length, 1);
});

test("independent actions are not merged by recipe or by a shared driver owner label", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  for (const jobId of ["proof-1", "proof-2"]) await ui.event({ type: "proof-job", ...proofJob(jobId) });
  for (const txFinal of ["tx-1", "tx-2"]) {
    await ui.event({ type: "transaction", ...transaction("submitted", { owner: "execute", txFinal, action: qname("GatherWood") }) });
  }
  assert.equal(ui.jobs().length, 4);
});

test("recovery coalesces retained proofs and transactions and keeps unmarked external jobs visible", async (context) => {
  const ui = await page(context, {
    objects: [knife()], transactions: [transaction("submitted", { owner: "proof-1", action: qname("GatherWood") })],
    proofJobs: new Map([["proof-1", proofJob("proof-1", "done", { txFinal: "0xabc", finishedAtMs: 2000 })]]),
  });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /GatherWood/);
  await ui.event({ type: "transaction", ...transaction("landed", { owner: "proof-1", action: qname("GatherWood"), settledAtMs: 3000 }) });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /succeeded/);
});

test("a recovered automatic starter transaction stays hidden by its owner's saved run ID", async (context) => {
  const ui = await page(context, {
    objects: [knife("pending")], transactions: [transaction()], startupRunId: "lost-run",
  });
  assert.equal(ui.jobs().length, 0);
  assert.equal(ui.backend.posts.length, 0);
  await ui.event({ type: "transaction", ...transaction("landed", { settledAtMs: 3000 }) });
  await ui.event({ type: "object", ...knife() });
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.jobs().length, 0);
});

test("failure to restore proof jobs prevents duplicate starter creation", async (context) => {
  const ui = await page(context, { proofJobError: true });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /Could not check active jobs/);
});

test("reconnect and sequence gaps discover activity missed while disconnected", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "object", ...knife() }, 1);
  ui.backend.proofJobs.set("missed-proof", proofJob("missed-proof"));
  await ui.event({ type: "object", ...knife() }, 3);
  assert.equal(ui.jobs().length, 1);
  ui.backend.states.set("finished", state("finished", "succeeded", { action: qname("GatherWood"), finishedAtMs: 3000 }));
  inventoryStream(ui).onopen();
  await flush();
  assert.equal(ui.jobs().length, 2);
  assert.match(ui.jobs().find((row) => row.textContent.includes("fini")).textContent, /succeeded/);
});

test("proof events received during recovery supersede the older snapshot", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  ui.backend.proofJobs.set("proof-1", proofJob("proof-1", "recording", { message: "Old progress" }));
  let release;
  ui.backend.activityGate = new Promise((resolve) => { release = resolve; });
  inventoryStream(ui).onopen();
  await flush();
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "recording", { message: "New progress" }) });
  ui.backend.activityGate = null;
  release();
  await flush();
  assert.equal(ui.jobs().length, 1);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.match(ui.document.querySelector(".run-log tr").textContent, /New progress/);
});

test("proof failure and transaction deletion end jobs and release local input reservations", async (context) => {
  const ui = await page(context, { objects: [knife(), wood("wood.dobj")] });
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "failed", { error: "Mining failed", finishedAtMs: 2000 }) });
  assert.equal(ui.jobs()[0].classList.contains("failed"), true);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.match(ui.document.querySelector("#details-body").textContent, /Mining failed/);
  await ui.event({ type: "transaction", ...transaction("recorded", { owner: "proof-2", inputFiles: ["wood.dobj"] }) });
  assert.equal(ui.dom.window.inputsAvailable(["wood.dobj"]), false);
  await ui.event({ type: "transaction-deleted", txFinal: "0xabc" });
  assert.equal(ui.jobs().length, 2);
  assert.equal(ui.jobs().every((row) => row.classList.contains("failed")), true);
  assert.equal(ui.dom.window.inputsAvailable(["wood.dobj"]), true);
});

test("a deleted transaction's completed duration stays fixed on later inventory events", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  let now = Date.now();
  ui.dom.window.Date.now = () => now;
  await ui.event({ type: "transaction", ...transaction("recorded", { recordedAtMs: now - 1000 }) });
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  await ui.event({ type: "transaction-deleted", txFinal: "0xabc" });
  const duration = () => [...ui.document.querySelector("#details-body .field-table").rows]
    .find((row) => row.cells[0].textContent === "duration").cells[1].textContent;
  assert.equal(duration(), "1s");
  now += 60000;
  await ui.event({ type: "object", ...knife() });
  assert.equal(duration(), "1s");
});

test("a lost run keeps following its durable transaction", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  ui.backend.states.set("external", state("external", "committing"));
  await ui.event({ type: "run-action-progress", runId: "external", phase: "commit", status: "running" });
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "external" }) });
  ui.backend.states.delete("external");
  await ui.dropStream("external");
  assert.equal(ui.jobs()[0].classList.contains("in-progress"), true);
  await ui.event({ type: "transaction", ...transaction("landed", { owner: "external", settledAtMs: 3000 }) });
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  assert.equal(ui.jobs().length, 1);
});

test("recovery ends unfinished proofs lost on daemon restart and follows any surviving transaction", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "proof-job", ...proofJob("lost-proof") });
  await ui.event({ type: "proof-job", ...proofJob("recorded-proof") });
  ui.backend.proofJobs.clear();
  ui.backend.transactions = [transaction("submitted", { owner: "recorded-proof", action: qname("GatherWood") })];
  inventoryStream(ui).onopen();
  await flush();
  assert.equal(ui.jobs().length, 2);
  const lost = ui.jobs().find((row) => row.textContent.includes("lost"));
  assert.equal(lost.classList.contains("failed"), true);
  lost.dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.match(ui.document.querySelector("#details-body").textContent, /no longer has this proof job/);
  assert.equal(ui.jobs().find((row) => row.textContent.includes("reco")).classList.contains("in-progress"), true);
});

test("expiry ends a separately proved action and older submission events cannot reopen it", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "proof-1" }) });
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "done", { action: qname("CraftPocketknife"), txFinal: "0xabc" }) });
  await ui.event({ type: "transaction", ...transaction("expired", { owner: "proof-1", reason: "Grounding aged out", settledAtMs: 3000 }) });
  assert.equal(ui.jobs().length, 1);
  assert.equal(ui.jobs()[0].classList.contains("failed"), true);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  assert.match(ui.document.querySelector("#details-body").textContent, /Grounding aged out/);
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "proof-1" }) });
  assert.equal(ui.jobs()[0].classList.contains("failed"), true);
});

test("external input paths use the managed object's basename in selection and details", async (context) => {
  const ui = await page(context, { objects: [knife(), wood("wood.dobj")] });
  await ui.event({ type: "proof-job", ...proofJob("proof-1", "proving", { inputObjectPaths: ["/external/path/wood.dobj"] }) });
  assert.equal(ui.dom.window.inputsAvailable(["wood.dobj"]), false);
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  const details = ui.document.querySelector("#details-body").textContent;
  assert.match(details, /\/Users\/player\/\.bitcraft\/objects\/wood\.dobj/);
  assert.doesNotMatch(details, /objects\/\/external/);
});

test("other plugins' activity stays out of Bitcraft Jobs", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  const action = qname("GatherWood", "other");
  ui.backend.states.set("other-run", state("other-run", "generateProof", { action }));
  await ui.event({ type: "run-action-progress", runId: "other-run", phase: "generateProof", status: "running" });
  await ui.event({ type: "proof-job", ...proofJob("other-proof", "proving", { action }) });
  await ui.event({ type: "transaction", ...transaction("submitted", { action }) });
  assert.equal(ui.jobs().length, 0);
});

test("independent runs overlap, including no-input actions, while local inputs are reserved before POST returns", async (context) => {
  const craft = { ...recipe("CraftStick"), totalInputs: [{ class: qname("Wood") }] };
  const ui = await page(context, { objects: [knife(), wood("a.dobj"), wood("b.dobj")], actions: [recipe(), craft] });
  let release;
  ui.backend.postGate = new Promise((resolve) => { release = resolve; });
  const first = ui.dom.window.startRun(craft, ["a.dobj"]);
  await flush();
  assert.equal(ui.dom.window.inputsAvailable(["a.dobj"]), false);
  assert.equal(ui.dom.window.inputsAvailable(["b.dobj"]), true);
  assert.equal(await ui.dom.window.startRun(craft, ["a.dobj"]), null);
  const second = ui.dom.window.startRun(craft, ["b.dobj"]);
  const empty = ui.dom.window.startRun(recipe(), []);
  await flush();
  assert.equal(ui.backend.posts.length, 3, "other inputs and no-input actions do not wait");
  ui.backend.postGate = null;
  release();
  await Promise.all([first, second, empty]);
  await ui.publish("run-1", "failed", { error: "Proof failed" });
  assert.equal(ui.dom.window.inputsAvailable(["a.dobj"]), true, "failure releases the local reservation");
  assert.equal(ui.dom.window.inputsAvailable(["b.dobj"]), false, "the other run keeps its inputs");
});

test("external holds update an open action menu, and release enables the input again", async (context) => {
  const craft = { ...recipe("CraftStick"), totalInputs: [{ class: qname("Wood") }] };
  const ui = await page(context, { objects: [knife(), wood("wood.dobj")], actions: [recipe(), craft] });
  const row = ui.document.querySelector('.object-row[data-file-name="wood.dobj"]');
  row.dispatchEvent(new ui.dom.window.MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  const item = () => [...ui.document.querySelectorAll(".context-menu .menu-item")]
    .find((item) => item.textContent.includes("CraftStick"));
  assert.equal(item().classList.contains("disabled"), false);
  const reads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("wood.dobj", { heldByRunId: "external" }) });
  assert.equal(item().classList.contains("disabled"), true);
  assert.equal(ui.document.querySelector('.object-row[data-file-name="wood.dobj"]').classList.contains("held"), true);
  await ui.event({ type: "object", ...wood("wood.dobj", { heldByRunId: null }) });
  assert.equal(item().classList.contains("disabled"), false);
  assert.equal(ui.backend.inventoryReads, reads);
});

for (const mode of ["main", "summary"]) {
  test(`${mode} inventory preserves events received during a stale snapshot, including removals and transaction ownership`, async (context) => {
    const ui = await page(context, {
      objects: [knife(), wood("old.dobj")],
      ...(mode === "summary" ? { url: "http://127.0.0.1:7727/bitcraft/?page=summary" } : {}),
    });
    let release;
    ui.backend.inventoryGate = new Promise((resolve) => { release = resolve; });
    inventoryStream(ui).onopen();
    await flush();
    await ui.event({ type: "object", ...wood("new.dobj", { status: "unknown" }) });
    await ui.event({ type: "transaction", ...transaction("submitted", { outputFiles: ["new.dobj"] }) });
    await ui.event({ type: "object", ...wood("old.dobj", { status: "nullified" }) });
    ui.backend.inventoryGate = null;
    release();
    await flush();
    if (mode === "main") {
      assert.ok(ui.document.querySelector('.object-row[data-file-name="new.dobj"]'));
      assert.equal(ui.document.querySelector('.object-row[data-file-name="old.dobj"]'), null);
    } else {
      assert.equal(ui.document.querySelector('[data-section="classes"] > summary').textContent, "Objects (1):");
      assert.equal([...ui.timers.values()].filter((timer) => timer.delay === 10000).length, 0,
        "the transaction received during the snapshot keeps its output out of fallback polling");
    }
    await ui.event({ type: "object", ...wood("new.dobj") });
    if (mode === "main") assert.equal(ui.document.title, "Bitcraft (1.00 BIT)");
    else assert.equal(ui.document.querySelector(".summary-total").textContent, "1.00 BIT");
    await ui.event({ type: "object-removed", fileName: "new.dobj" });
    if (mode === "main") assert.equal(ui.document.querySelector('.object-row[data-file-name="new.dobj"]'), null);
    else assert.equal(ui.document.querySelector(".summary-total").textContent, "0.00 BIT");
  });
}

test("sequence gaps and reconnects recover missed inventory changes, including a daemon sequence reset", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "object", ...wood("a.dobj") }, 1);
  ui.backend.objects.push(wood("missed.dobj"));
  const reads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("a.dobj") }, 3);
  assert.equal(ui.backend.inventoryReads, reads + 1);
  assert.ok(ui.document.querySelector('.object-row[data-file-name="missed.dobj"]'));
  inventoryStream(ui).onopen();
  await flush();
  ui.backend.objects.push(wood("after-restart.dobj"));
  inventoryStream(ui).onopen();
  await flush();
  const resetReads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("a.dobj", { heldByRunId: "new-daemon" }) }, 1);
  assert.equal(ui.backend.inventoryReads, resetReads, "a restarted daemon begins a new sequence");
  assert.ok(ui.document.querySelector('.object-row[data-file-name="after-restart.dobj"]'));
});

test("recorded output updates need no inventory polling in the summary window", async (context) => {
  const ui = await page(context, {
    url: "http://127.0.0.1:7727/bitcraft/?page=summary",
    objects: [knife("pending")], transactions: [transaction()],
  });
  const reads = ui.backend.inventoryReads;
  assert.equal([...ui.timers.values()].filter((timer) => timer.delay === 10000).length, 0);
  await ui.event({ type: "run-action-progress", runId: "external", phase: "commit", status: "running" });
  await ui.event({ type: "object", ...knife() });
  assert.equal(ui.backend.inventoryReads, reads);
  assert.equal(ui.document.querySelector('[data-section="classes"] > summary').textContent, "Objects (1):");
});

for (const status of ["submitted", "recorded"]) {
  test(`startup resumes a durable ${status} transaction after its run is lost`, async (context) => {
    const ui = await page(context, { objects: [knife("pending")], transactions: [transaction(status)] });
    assert.equal(ui.backend.posts.length, 0);
    assert.deepEqual(ui.backend.discards, []);
    assert.equal(ui.jobs().length, 1, "the externally started transaction appears in Jobs");
    assert.equal(ui.progress(), 3);
    assert.match(ui.title(), /Connecting to Bitcraft network/);
    assert.equal(ui.backend.submissions.length, status === "recorded" ? 1 : 0);
    await ui.event({ type: "transaction", ...transaction("landed") });
    await ui.event({ type: "object", ...knife() });
    assert.equal(ui.app.hidden, false);
    assert.equal(ui.progress(), 4);
  });
}

test("a submission attempt with an unknown outcome is followed without resubmitting or replacing its outputs", async (context) => {
  const ui = await page(context, {
    objects: [knife("unknown")], transactions: [transaction("recorded", { submitAttemptedAtMs: 1 })],
  });
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.backend.submissions.length, 0);
  assert.deepEqual(ui.backend.discards, []);
  await ui.event({ type: "transaction", ...transaction("expired", { reason: "Grounding aged out" }) });
  await ui.event({ type: "object-removed", fileName: "pocketknife.dobj" });
  assert.match(ui.document.querySelector("#startup-error").textContent, /Grounding aged out/);
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1, "a new proof is made only after expiry and an explicit retry");
});

test("failure to read durable transactions prevents startup from discarding outputs or creating duplicates", async (context) => {
  const ui = await page(context, { objects: [knife("unknown")], transactionError: true });
  assert.equal(ui.backend.posts.length, 0);
  assert.deepEqual(ui.backend.discards, []);
  assert.equal(ui.app.hidden, true);
  ui.backend.transactionError = false;
  ui.backend.transactions = [transaction()];
  await ui.retry();
  assert.equal(ui.backend.posts.length, 0);
  assert.deepEqual(ui.backend.discards, []);
});

test("failed submission of an existing starter transaction stays recoverable without making another proof", async (context) => {
  const ui = await page(context, { objects: [knife("unknown")], transactions: [transaction("recorded")], submitError: true });
  assert.equal(ui.backend.posts.length, 0);
  assert.deepEqual(ui.backend.discards, []);
  assert.equal(ui.document.querySelector("#startup-error").hidden, false);
  await ui.event({ type: "object", ...knife("unknown") });
  assert.equal(ui.document.querySelector("#startup-error").hidden, false, "object updates preserve the submission error");
  ui.backend.submitError = false;
  await ui.retry();
  assert.equal(ui.backend.posts.length, 0);
  assert.equal(ui.backend.submissions.length, 2);
  assert.equal(ui.document.querySelector("#startup-error").hidden, true);
});

test("deleting a starter transaction requires retry before making a replacement", async (context) => {
  const ui = await page(context, { objects: [knife("unknown")], transactions: [transaction()] });
  await ui.event({ type: "transaction-deleted", txFinal: "0xabc" });
  await ui.event({ type: "object-removed", fileName: "pocketknife.dobj" });
  assert.equal(ui.backend.posts.length, 0);
  assert.match(ui.document.querySelector("#startup-error").textContent, /no longer available/);
  await ui.retry();
  assert.equal(ui.backend.posts.length, 1);
});

test("completed job details use spent input paths and the grounding root from the current run result", async (context) => {
  const craft = { ...recipe("CraftStick"), totalInputs: [{ class: qname("Wood") }] };
  const ui = await page(context, { objects: [knife(), wood("wood.dobj")], actions: [recipe(), craft] });
  await ui.dom.window.startRun(craft, ["wood.dobj"]);
  await ui.publish("run-1", "succeeded", {
    result: { oldRoot: "0xgrounding", inputFiles: ["wood.dobj"], outputFiles: [] },
  });
  ui.jobs()[0].dispatchEvent(new ui.dom.window.MouseEvent("mousedown", { button: 0, bubbles: true }));
  const details = ui.document.querySelector("#details-body").textContent;
  assert.match(details, /\.nullified\/wood\.dobj/);
  assert.match(details, /grounding root0xgrounding/);
  assert.doesNotMatch(details, /new root/);
});


test("pruning a completed transaction preserves its successful job", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "transaction", ...transaction("submitted", { owner: "external", action: qname("GatherWood") }) });
  await ui.event({ type: "transaction", ...transaction("landed", { owner: "external", action: qname("GatherWood"), settledAtMs: 3000 }) });
  assert.equal(ui.jobs().length, 1);
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  await ui.event({ type: "transaction-pruned", txFinal: "0xabc" });
  assert.equal(ui.backend.transactions.length, 0);
  assert.match(ui.jobs()[0].textContent, /succeeded/);
  assert.equal(ui.jobs()[0].classList.contains("failed"), false);
});
