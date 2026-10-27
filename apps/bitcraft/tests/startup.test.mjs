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
        if (parsed.pathname === "/actions") return respond(backend.actions);
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
    } else if (payload.type === "transaction-deleted") {
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
  assert.equal(ui.sources.find((source) => source.url.includes("/runs/")).closed, true, "the stream closing delivers the terminal state");
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

test("each startup step stays up for its minimum time, even when the work behind it is done", async (context) => {
  const ui = await page(context, { holdSteps: true });
  assert.equal(ui.backend.posts.length, 1, "the work goes ahead while a step is held");
  assert.match(ui.title(), /Opening Bitcraft/);
  assert.equal(ui.progress(), 0);
  assert.equal(ui.holds.size, 1);
  assert.equal([...ui.holds.values()][0].delay, 1000);
  await ui.elapse();
  assert.match(ui.title(), /Creating your first pocketknife/);
  assert.equal(ui.progress(), 1);
  ui.backend.objects = [knife("pending")];
  await ui.publish("run-1", "committing");
  assert.match(ui.title(), /Creating your first pocketknife/, "confirmation waits for the proof step's minimum");
  await ui.elapse();
  assert.match(ui.title(), /Waiting for confirmation/);
  assert.equal(ui.progress(), 2);
  await ui.complete();
  assert.equal(ui.splash.hidden, false, "the inventory waits for the confirmation step's minimum");
  assert.equal(ui.app.hidden, true);
  await ui.elapse();
  assert.equal(ui.splash.hidden, true);
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.progress(), 3);
});

test("a held startup step skips steps the startup did not reach", async (context) => {
  const ui = await page(context, { holdSteps: true, objects: [knife()] });
  assert.match(ui.title(), /Opening Bitcraft/);
  assert.equal(ui.app.hidden, true, "an existing knife still shows the first step for its minimum");
  await ui.elapse();
  assert.equal(ui.app.hidden, false);
  assert.equal(ui.holds.size, 0);
});

test("errors and retries replace a held startup step at once", async (context) => {
  const ui = await page(context, { holdSteps: true, inventoryError: true });
  assert.match(ui.title(), /could not finish setup/, "an error on the step on display is not held back");
  assert.equal(ui.progress(), 0);
  ui.backend.inventoryError = false;
  await ui.retry();
  assert.match(ui.title(), /Opening Bitcraft/);
  assert.equal(ui.holds.size, 1, "retry restarts the first step's minimum");
  await ui.elapse();
  assert.match(ui.title(), /Creating your first pocketknife/);
  await ui.publish("run-1", "failed", { error: "Proof failed" });
  assert.match(ui.document.querySelector("#startup-error").textContent, /Proof failed/);
  assert.equal(ui.document.querySelector("#startup-actions").hidden, false);
  await ui.retry();
  assert.match(ui.title(), /Opening Bitcraft/, "retry goes back to the first step without waiting");
  assert.equal(ui.progress(), 0);
});

test("the pocketknife steps show the pocketknife's icon from the presentation pack", async (context) => {
  const ui = await page(context, {
    presentation: {
      "index.json": { packs: ["bitcraft/pack.json"] },
      "bitcraft/pack.json": { format: 1, plugin: "bitcraft", classes: { Pocketknife: { icon: "assets/pocketknife.png" } } },
    },
  });
  const icon = ui.document.querySelector("#startup-class-icon");
  assert.match(ui.title(), /Creating your first pocketknife/);
  assert.equal(icon.hidden, false);
  assert.equal(icon.src, "http://127.0.0.1:7727/bitcraft/presentation/bitcraft/assets/pocketknife.png");
  await ui.publish("run-1", "failed", { error: "Proof failed" });
  assert.equal(icon.hidden, true, "the setup error shows a blank page");
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
  await flush();
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
  const events = ui.sources.find((source) => source.url.endsWith("/events"));
  const eventReads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...item("wood-3.dobj", "Wood") });
  assert.equal(ui.backend.inventoryReads, eventReads, "an object event needs no inventory request");
  assert.deepEqual(fields("classes"), ["bitcraft::Pocketknife: 1", "bitcraft::Wood: 3"]);
  assert.equal(document.querySelector('[data-section="general"]').open, false);

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
    { url: "https://bitcraft.sh/", name: "_blank" },
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
  ui.dom.window.dispatchEvent(new ui.dom.window.Event("focus"));
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
      .map((fileName) => document.querySelector(`.object-row[title="${fileName}"]`));
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
  const row = ui.document.querySelector('.object-row[title="wood.dobj"]');
  row.dispatchEvent(new ui.dom.window.MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  const item = () => [...ui.document.querySelectorAll(".context-menu .menu-item")]
    .find((item) => item.textContent.includes("CraftStick"));
  assert.equal(item().classList.contains("disabled"), false);
  const reads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("wood.dobj", { heldByRunId: "external" }) });
  assert.equal(item().classList.contains("disabled"), true);
  assert.equal(ui.document.querySelector('.object-row[title="wood.dobj"]').classList.contains("held"), true);
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
      assert.ok(ui.document.querySelector('.object-row[title="new.dobj"]'));
      assert.equal(ui.document.querySelector('.object-row[title="old.dobj"]'), null);
    } else {
      assert.match(ui.document.querySelector(".summary-subtitle").textContent, /1 live object in total/);
      assert.equal([...ui.timers.values()].filter((timer) => timer.delay === 10000).length, 0,
        "the transaction received during the snapshot keeps its output out of fallback polling");
    }
    await ui.event({ type: "object", ...wood("new.dobj") });
    if (mode === "main") assert.match(ui.document.title, /Total Bit Value: 1/);
    else assert.match(ui.document.querySelector(".summary-total").textContent, /1 bit/);
    await ui.event({ type: "object-removed", fileName: "new.dobj" });
    if (mode === "main") assert.equal(ui.document.querySelector('.object-row[title="new.dobj"]'), null);
    else assert.match(ui.document.querySelector(".summary-total").textContent, /0 bits/);
  });
}

test("sequence gaps and reconnects recover missed inventory changes, including a daemon sequence reset", async (context) => {
  const ui = await page(context, { objects: [knife()] });
  await ui.event({ type: "object", ...wood("a.dobj") }, 1);
  ui.backend.objects.push(wood("missed.dobj"));
  const reads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("a.dobj") }, 3);
  assert.equal(ui.backend.inventoryReads, reads + 1);
  assert.ok(ui.document.querySelector('.object-row[title="missed.dobj"]'));
  inventoryStream(ui).onopen();
  await flush();
  ui.backend.objects.push(wood("after-restart.dobj"));
  inventoryStream(ui).onopen();
  await flush();
  const resetReads = ui.backend.inventoryReads;
  await ui.event({ type: "object", ...wood("a.dobj", { heldByRunId: "new-daemon" }) }, 1);
  assert.equal(ui.backend.inventoryReads, resetReads, "a restarted daemon begins a new sequence");
  assert.ok(ui.document.querySelector('.object-row[title="after-restart.dobj"]'));
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
  assert.match(ui.document.querySelector(".summary-subtitle").textContent, /1 live object in total/);
});

for (const status of ["submitted", "recorded"]) {
  test(`startup resumes a durable ${status} transaction after its run is lost`, async (context) => {
    const ui = await page(context, { objects: [knife("pending")], transactions: [transaction(status)] });
    assert.equal(ui.backend.posts.length, 0);
    assert.deepEqual(ui.backend.discards, []);
    assert.equal(ui.jobs().length, 1, "the externally started transaction appears in Jobs");
    assert.equal(ui.progress(), 2);
    assert.match(ui.title(), /Waiting for confirmation/);
    assert.equal(ui.backend.submissions.length, status === "recorded" ? 1 : 0);
    await ui.event({ type: "transaction", ...transaction("landed") });
    await ui.event({ type: "object", ...knife() });
    assert.equal(ui.app.hidden, false);
    assert.equal(ui.progress(), 3);
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
