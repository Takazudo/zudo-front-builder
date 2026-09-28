import { beforeEach, expect, it, vi } from "vitest";
import { h, signal, flush } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate } from "../../zudo-react/client.js";
import type { Diagnostic } from "../../zudo-react/index.js";
const identity = { component: "Demo", build: "b1" };
beforeEach(() => document.body.replaceChildren());
it("updates adopted attributes and native listeners, then releases them", async () => {
  const title = signal<string | null>("old");
  const hidden = signal(false);
  const style = signal("color:red;");
  const click = vi.fn();
  function Demo() {
    return h("button", { title, hidden, style, "on:click": click, children: "Go" });
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  const button = container.querySelector("button")!;
  const handle = hydrate(h(Demo, {}), container, { identity })!;
  expect(container.querySelector("button")).toBe(button);
  button.click();
  expect(click).toHaveBeenCalledTimes(1);
  title.value = null;
  hidden.value = true;
  style.value = "color:blue;";
  await flush();
  expect(button.hasAttribute("title")).toBe(false);
  expect(button.hasAttribute("hidden")).toBe(true);
  expect(button.getAttribute("style")).toBe("color:blue;");
  handle.dispose();
  button.click();
  expect(click).toHaveBeenCalledTimes(1);
});
it("reports rejected async listeners without disturbing other roots", async () => {
  const diagnostics: Diagnostic[] = [];
  function Demo() {
    return h("button", { "on:click": () => Promise.reject(new Error("boom")), children: "Go" });
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  hydrate(h(Demo, {}), container, { identity, report: (item) => diagnostics.push(item) });
  container.querySelector("button")!.click();
  await Promise.resolve();
  await Promise.resolve();
  expect(diagnostics.at(-1)?.code).toBe("ZR_EVENT_ERROR");
});
