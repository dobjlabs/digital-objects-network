import assert from "node:assert/strict";
import test from "node:test";
import { JSDOM } from "jsdom";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { buildModule, importModule } from "./buildModule.mjs";

const code = await buildModule(
  new URL("../src/features/context/ContextPanel.tsx", import.meta.url),
  { external: ["react", "react/jsx-runtime"] },
);
const { ContextPanel } = await importModule(code);

test("inventory updates invalidate bound inputs without restoring them later", async (context) => {
  const dom = new JSDOM("<!doctype html><div id='root'></div>");
  const globals = { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true };
  const descriptors = new Map(Object.keys(globals).map((key) => [
    key, Object.getOwnPropertyDescriptor(globalThis, key),
  ]));
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  }
  let root;
  context.after(async () => {
    if (root) await act(() => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const className = { pluginName: "demo", name: "Log" };
  const actionName = { pluginName: "demo", name: "UseLog" };
  const object = {
    fileName: "demo_Log_123.dobj",
    contentHash: "123",
    class: className,
    classHash: "abc",
    emoji: "",
    status: "live",
    txHash: null,
    fields: {},
  };
  const action = {
    action: actionName,
    hash: "def",
    emoji: "",
    description: "Use a log",
    totalInputs: [{ class: className, hash: "abc" }],
  };
  const container = document.getElementById("root");
  root = createRoot(container);
  const runRequests = [];
  const props = {
    selection: { kind: "action", action: actionName },
    objectsDirPath: "/objects",
    actions: [action],
    onClearSelection() {},
    async onRunProof(input) { runRequests.push(input); },
    proofRunning: false,
    proofStatus: "idle",
  };
  const render = async (objects, actions = [action]) => {
    await act(() => root.render(createElement(ContextPanel, { ...props, objects, actions })));
  };
  const bind = async () => {
    const select = container.querySelector("select");
    await act(() => {
      select.value = object.fileName;
      select.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
    assert.equal(select.value, object.fileName);
    assert.equal(container.querySelector(".method-execute").disabled, false);
  };
  const assertUnbound = () => {
    assert.equal(container.querySelector("select").value, "");
    assert.equal(container.querySelector(".method-execute").disabled, true);
    assert.equal(container.querySelector(".method-arg-drop").textContent, "drag .dobj here");
  };

  await render([object]);
  await bind();
  await render([{ ...object }]);
  assert.equal(container.querySelector("select").value, object.fileName);
  await act(() => container.querySelector(".method-execute").click());
  assert.deepEqual(runRequests[0].inputBindings, [{ objectPath: object.fileName, label: object.fileName }]);

  for (const updatedObjects of [
    [{ ...object, status: "nullified" }],
    [],
    [{ ...object, class: { pluginName: "other-plugin", name: "Log" } }],
  ]) {
    await render(updatedObjects);
    assertUnbound();
    await act(() => container.querySelector(".method-execute").click());
    assert.equal(runRequests.length, 1);
    await render([object]);
    assertUnbound();
    await bind();
  }
  await render([object], [{ ...action, totalInputs: [{ class: { ...className, name: "Wood" }, hash: "ghi" }] }]);
  assertUnbound();
  await render([object]);
  assertUnbound();
});
