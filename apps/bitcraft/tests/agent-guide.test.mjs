import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { JSDOM, VirtualConsole } from "jsdom";

const script = await readFile(new URL("../shell/agent.js", import.meta.url), "utf8");
const html = (await readFile(new URL("../shell/agent.html", import.meta.url), "utf8"))
  .replace('<script src="agent.js" defer></script>', "")
  .replace("</body>", () => `<script>${script}</script></body>`);
const guide = {
  prompt: "Help me play Bitcraft.\nRead /Applications/Bitcraft.app/skills/SKILL.md.\nUse /Applications/Bitcraft.app/bitcraft-dobj from /data/Alice's game.",
  error: null,
};

function page(context, { clipboard = async () => {}, error = null, fallback = false } = {}) {
  const errors = [];
  const virtualConsole = new VirtualConsole();
  virtualConsole.on("jsdomError", (error) => errors.push(error));
  const dom = new JSDOM(html, {
    runScripts: "dangerously", virtualConsole,
    beforeParse(window) {
      window.bitcraftAgentGuide = { ...guide, error };
      Object.defineProperty(window.navigator, "clipboard", { value: { writeText: clipboard } });
      window.document.execCommand = () => fallback;
    },
  });
  context.after(() => { dom.window.close(); assert.deepEqual(errors, []); });
  return dom;
}

test("the manual guide copies the full player prompt", async (context) => {
  const copied = [];
  const dom = page(context, { clipboard: async (value) => copied.push(value) });
  const { document } = dom.window;
  assert.equal(document.getElementById("agent-error").hidden, true);
  document.getElementById("copy").click();
  await new Promise(setImmediate);
  assert.deepEqual(copied, [guide.prompt]);
  assert.equal(document.getElementById("copy-status").textContent, "Copied");
});

test("failed deep links keep the prompt usable and reopening the guide clears the error", async (context) => {
  const error = "Claude desktop is not available. <Try another agent>";
  const dom = page(context, { error });
  const { document } = dom.window;
  assert.equal(document.getElementById("agent-error").hidden, false);
  assert.equal(document.getElementById("agent-error").textContent, error);
  assert.equal(document.getElementById("agent-error").children.length, 0);
  assert.equal(document.getElementById("prompt").value, guide.prompt);
  assert.equal(document.getElementById("copy").disabled, false);
  dom.window.bitcraftAgentGuide = { ...guide, prompt: "Use /data/new game." };
  dom.window.dispatchEvent(new dom.window.Event("bitcraft-agent-guide-updated"));
  assert.equal(document.getElementById("agent-error").hidden, true);
  assert.equal(document.getElementById("prompt").value, "Use /data/new game.");
});

test("clipboard failures leave the prompt selected for manual copying", async (context) => {
  const dom = page(context, { clipboard: async () => { throw new Error("Unavailable"); } });
  const { document } = dom.window;
  document.getElementById("copy").click();
  await new Promise(setImmediate);
  const prompt = document.getElementById("prompt");
  assert.equal(document.activeElement, prompt);
  assert.equal(prompt.selectionStart, 0);
  assert.equal(prompt.selectionEnd, guide.prompt.length);
  assert.match(document.getElementById("copy-status").textContent, /copy it manually/);
});
