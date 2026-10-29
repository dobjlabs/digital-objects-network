import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const pageHtml = await readFile(new URL("../ui/index.html", import.meta.url), "utf8");
const pageCss = pageHtml.match(/<style>([\s\S]*?)<\/style>/)[1];
const shellCss = await readFile(new URL("../shell/style.css", import.meta.url), "utf8");

function rootVariables(css, selector) {
  const escaped = selector.replace(/[[\]"]/g, "\\$&");
  const block = css.match(new RegExp(`(?:^|\\n)\\s*${escaped}\\s*\\{([^}]*)\\}`))[1];
  return Object.fromEntries([...block.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)]
    .map(([, name, value]) => [name, value.trim()]));
}

function themes(css) {
  const light = rootVariables(css, ":root");
  const overrides = rootVariables(css, ':root[data-theme="dark"]');
  for (const name of Object.keys(overrides)) assert.ok(name in light, `${name} has no light value`);
  return { light, dark: { ...light, ...overrides } };
}

test("the desktop shell's theme variables match the game page's in both themes", () => {
  const page = themes(pageCss);
  const shell = themes(shellCss);
  for (const name of Object.keys(shell.light)) {
    assert.equal(shell.light[name], page.light[name], `light ${name}`);
    assert.equal(shell.dark[name], page.dark[name], `dark ${name}`);
  }
});
