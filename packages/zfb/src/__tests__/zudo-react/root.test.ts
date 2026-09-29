import { beforeEach, expect, it } from "vitest";
import { h } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
const identity = { component: "Demo", build: "b1" };
beforeEach(() => document.body.replaceChildren());
it("stores a shared-symbol handle and stale disposal cannot remove a replacement", () => {
  function Demo() {
    return h("p", { children: "value" });
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  const first = hydrate(h(Demo, {}), container, { identity })!;
  const key = Symbol.for("@takazudo/zfb/zudo-react/root-v1");
  expect((container as unknown as Record<symbol, unknown>)[key]).toBe(first);
  first.dispose();
  const second = mount(h(Demo, {}), container, { identity })!;
  const replacement = container.querySelector("p")!;
  first.unmount();
  expect(container.querySelector("p")).toBe(replacement);
  second.unmount();
  expect(container.childNodes).toHaveLength(0);
});
