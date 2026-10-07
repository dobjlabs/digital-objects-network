import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer as createHttpServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

test("dev proxy delivers a large UTF-8 catalogue and streams SSE", { timeout: 15000 }, async () => {
  const actions = Array.from({ length: 600 }, (_, index) => ({
    id: `action-${index}`,
    description: "A catalogue entry with unicode: \u{1F6F5} \u{1F332}".repeat(5),
  }));
  const body = Buffer.from(JSON.stringify(actions));
  assert.ok(body.length > 128 * 1024);
  const requests = [];
  let eventResponse;
  const upstream = createHttpServer((request, response) => {
    requests.push({ path: request.url, origin: request.headers.origin, host: request.headers.host });
    if (request.url === "/events") {
      eventResponse = response;
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      response.write('data: {"status":"running"}\n\n');
    } else {
      response.writeHead(200, {
        "Content-Type": "application/json",
        "Content-Length": body.length,
        Connection: "close",
      });
      response.write(body.subarray(0, 65536));
      response.end(body.subarray(65536));
    }
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const previousUrl = process.env.VITE_DOBJD_URL;
  process.env.VITE_DOBJD_URL = `http://127.0.0.1:${upstream.address().port}`;
  const cache = await mkdtemp(join(tmpdir(), "dobj-vite-proxy-"));
  let vite;
  try {
    vite = await createServer({
      root: fileURLToPath(new URL("..", import.meta.url)),
      cacheDir: cache,
      logLevel: "silent",
      server: { host: "127.0.0.1", port: 0 },
    });
    await vite.listen();
    const base = `http://127.0.0.1:${vite.httpServer.address().port}/api`;
    const options = { headers: { Origin: "https://forwarded-ui.example" }, signal: AbortSignal.timeout(5000) };
    const response = await fetch(`${base}/actions`, options);
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("content-length"), String(body.length));
    assert.equal(response.headers.get("connection"), "keep-alive");
    const received = Buffer.from(await response.arrayBuffer());
    assert.deepEqual(received, body);
    assert.deepEqual(JSON.parse(received.toString()), actions);

    const events = await fetch(`${base}/events`, options);
    assert.equal(events.headers.get("content-type"), "text/event-stream");
    const reader = events.body.getReader();
    const first = await reader.read();
    assert.equal(Buffer.from(first.value).toString(), 'data: {"status":"running"}\n\n');
    // The first event must arrive while the upstream stream remains open.
    assert.equal(eventResponse.writableEnded, false);
    eventResponse.end('data: {"status":"done"}\n\n');
    const remaining = [];
    for (;;) {
      const next = await reader.read();
      if (next.done) break;
      remaining.push(Buffer.from(next.value));
    }
    assert.equal(Buffer.concat(remaining).toString(), 'data: {"status":"done"}\n\n');
    assert.deepEqual(requests, ["/actions", "/events"].map((path) => ({
      path,
      origin: undefined,
      host: `127.0.0.1:${upstream.address().port}`,
    })));

    // Cursor's tunnel exits as soon as the remote socket closes, even if
    // stdout still has queued output. Read slowly to exercise that boundary.
    const bridge = spawn(process.execPath, ["-e", `
      const net = require('node:net');
      const socket = net.createConnection({host: '127.0.0.1', port: ${vite.httpServer.address().port}});
      socket.pipe(process.stdout);
      process.stdin.pipe(socket);
      socket.on('close', () => process.exit(0));
    `], { stdio: ["pipe", "pipe", "pipe"] });
    const exited = once(bridge, "exit");
    const forwardTimeout = setTimeout(() => {
      bridge.stdout.destroy(new Error("port-forwarded response must arrive in full"));
    }, 5000);
    try {
      bridge.stdin.write("GET /api/actions HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n");
      await delay(100);
      const parts = [];
      let received;
      for await (const chunk of bridge.stdout) {
        parts.push(chunk);
        const raw = Buffer.concat(parts);
        const separator = raw.indexOf("\r\n\r\n");
        if (separator !== -1 && raw.length - separator - 4 >= body.length) {
          received = raw.subarray(separator + 4);
          break;
        }
        await delay(20);
      }
      assert.deepEqual(received, body, "port-forwarded response must arrive in full");
    } finally {
      clearTimeout(forwardTimeout);
      bridge.kill();
      await exited;
    }
  } finally {
    eventResponse?.destroy();
    vite?.httpServer.closeAllConnections();
    await vite?.close();
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
    await rm(cache, { recursive: true, force: true });
    if (previousUrl === undefined) delete process.env.VITE_DOBJD_URL;
    else process.env.VITE_DOBJD_URL = previousUrl;
  }
});
