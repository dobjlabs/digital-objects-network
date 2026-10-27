import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const uiRoot = fileURLToPath(new URL("../ui/", import.meta.url));
const shellRoot = fileURLToPath(new URL("../shell/", import.meta.url));
const sessions = new Map();
const port = Number(process.env.BITCRAFT_PREVIEW_PORT ?? 4171);
const qname = (name) => ({ pluginName: "bitcraft", name });
const actions = [{
  action: qname("CraftPocketknife"), totalInputs: [],
  totalOutputs: [{ class: qname("Pocketknife"), hash: "preview" }],
  mutatedObjects: [], predicateSource: "", description: "Create a pocketknife",
}];
const contentTypes = {
  ".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
  ".json": "application/json", ".png": "image/png", ".svg": "image/svg+xml",
  ".md": "text/plain; charset=utf-8",
};
function reply(response, value, type = "application/json", status = 200) {
  response.writeHead(status, { "Content-Type": type, "Cache-Control": "no-store" });
  response.end(type === "application/json" && !Buffer.isBuffer(value) ? JSON.stringify(value) : value);
}
function runState(run) {
  const elapsed = Date.now() - run.createdAtMs;
  const status = run.fail && elapsed >= 5000 ? "failed"
    : elapsed >= 11000 ? "succeeded" : elapsed >= 5000 ? "committing"
      : elapsed >= 1000 ? "generateProof" : "queued";
  return {
    runId: run.runId, action: qname("CraftPocketknife"), status,
    inputObjectPaths: [], createdAtMs: run.createdAtMs, progress: [],
    finishedAtMs: ["failed", "succeeded"].includes(status) ? Date.now() : null,
    error: status === "failed" ? "The network could not confirm your pocketknife. Please try again." : null,
    result: status === "succeeded" ? { outputFiles: [`pocketknife-${run.runId}.dobj`], nullifiedFiles: [] } : null,
  };
}
function knife(fileName, status) {
  return {
    fileName, class: qname("Pocketknife"), classHash: "preview", contentHash: "preview",
    fileSize: 1824, status, fields: { kvs: [] }, heldByRunId: null,
  };
}
async function asset(response, root, path) {
  const resolved = resolve(root, path);
  if (!resolved.startsWith(resolve(root) + sep)) return reply(response, {}, "application/json", 404);
  try {
    reply(response, await readFile(resolved), contentTypes[extname(resolved)] ?? "application/octet-stream");
  } catch {
    reply(response, {}, "application/json", 404);
  }
}
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, `http://127.0.0.1:${port}`);
    if (url.pathname === "/") {
      return reply(response, `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
        <title>Bitcraft startup preview</title><style>body{font:16px/1.6 system-ui;max-width:600px;margin:80px auto;padding:24px;background:#fafafa;color:#1f1f1f}a{color:#6d28d9}li{margin:16px 0}</style>
        <h1>Bitcraft startup preview</h1><p>This preview uses a simulated daemon. It creates no real objects or proofs.</p><ul>
        <li><a href="/cryptography/">Desktop shell: preparing artifacts</a></li>
        <li><a href="/first-launch/">First launch</a> &mdash; proof, confirmation, then inventory (about 11 seconds)</li>
        <li><a href="/existing/">Existing inventory</a> &mdash; opens after one second</li>
        <li><a href="/failure/">Setup failure</a> &mdash; error after 5 seconds; retry succeeds</li></ul>`, "text/html; charset=utf-8");
    }
    const scenario = url.pathname.match(/^\/(first-launch|existing|failure)\/$/);
    if (scenario) {
      const sessionId = randomUUID();
      sessions.set(sessionId, { existing: scenario[1] === "existing", failNext: scenario[1] === "failure", runs: [] });
      const html = (await readFile(resolve(uiRoot, "index.html"), "utf8"))
        .replace("<head>", `<head><base href="/ui/"><meta name="dobjd-api-url" content="/api/${sessionId}">`);
      return reply(response, html, "text/html; charset=utf-8");
    }
    if (url.pathname.startsWith("/cryptography/")) {
      return asset(response, shellRoot, url.pathname.slice("/cryptography/".length) || "index.html");
    }
    if (url.pathname.startsWith("/ui/")) return asset(response, uiRoot, url.pathname.slice(4));
    const api = url.pathname.match(/^\/api\/([^/]+)(\/.*)$/);
    const session = api && sessions.get(api[1]);
    if (!session) return reply(response, {}, "application/json", 404);
    const path = api[2];
    if (path === "/settings") return reply(response, {});
    if (path === "/objects/dir") return reply(response, { path: "Bitcraft / objects" });
    if (path === "/objects/unspent") {
      const objects = session.existing ? [knife("pocketknife.dobj", "live")] : [];
      for (const run of session.runs) {
        const state = runState(run);
        if (["committing", "succeeded"].includes(state.status)) {
          objects.push(knife(`pocketknife-${run.runId}.dobj`, state.status === "succeeded" ? "live" : "pending"));
        }
      }
      return reply(response, objects);
    }
    if (path === "/actions") return reply(response, actions);
    if (path === "/runs" && request.method === "POST") {
      let body = "";
      for await (const chunk of request) body += chunk;
      const input = JSON.parse(body).input;
      if (input.action.name !== "CraftPocketknife") return reply(response, {}, "application/json", 400);
      const run = { runId: randomUUID(), createdAtMs: Date.now(), fail: session.failNext };
      session.failNext = false;
      session.runs.push(run);
      return reply(response, { runId: run.runId, createdAtMs: run.createdAtMs, status: "queued" });
    }
    if (path === "/runs") {
      return reply(response, session.runs.map(runState).filter((state) => !["failed", "succeeded"].includes(state.status)));
    }
    const runPath = path.match(/^\/runs\/([^/]+)(\/events)?$/);
    const run = runPath && session.runs.find((run) => run.runId === runPath[1]);
    if (run) {
      if (!runPath[2]) return reply(response, runState(run));
      response.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-store" });
      const send = () => response.write(`data: ${JSON.stringify(runState(run))}\n\n`);
      send();
      const timer = setInterval(send, 1000);
      response.on("close", () => clearInterval(timer));
      return;
    }
    reply(response, {}, "application/json", 404);
  } catch (error) {
    console.error(error);
    if (!response.headersSent) reply(response, {}, "application/json", 500);
    else response.end();
  }
});
server.listen(port, "127.0.0.1", () => {
  console.log(`Bitcraft startup preview: http://127.0.0.1:${port}`);
  console.log("Simulated daemon only; no real objects, proofs, or network transactions.");
});
