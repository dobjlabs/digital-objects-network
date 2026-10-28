import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const html = await readFile(new URL("../ui/index.html", import.meta.url), "utf8");

test("the page has a bare <head> for dobjd to inject its <base> and API origin into", () => {
  assert.ok(html.includes("<head>"));
});

test("the page takes its API origin from dobjd's meta tag", () => {
  assert.ok(html.includes('meta[name="dobjd-api-url"]'));
});
