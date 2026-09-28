// @vitest-environment node
import { describe, expect, it } from "vitest";

import { Fragment, getScope, h, signal, flush } from "../../zudo-react/index.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

describe("R-A01 server determinism and isolation", () => {
  it("renders the same component, props, and island marker identifiers to identical bytes", () => {
    function Profile({ label }: { label: string }) {
      return h("p", { title: label }, label);
    }

    const node = islandRoot(h(Profile, { label: "A & B" }), {
      identity: { component: "Profile", build: "build-1" },
    });
    const expected =
      '<div data-zfb-island="Profile" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="build-1" data-props="{&quot;label&quot;:&quot;A &amp; B&quot;}"><!--zr:1:0:c--><p title="A &amp; B">A &amp; B</p><!--/zr:1:0--></div>';

    expect(renderToString(node)).toBe(expected);
    expect(renderToString(node)).toBe(expected);
  });

  it("keeps request-local component signals isolated when instances alternate", () => {
    function PerInstance({ label }: { label: string }) {
      const local = signal(label);
      local.value = `${local.value}!`;
      return h("p", null, local);
    }

    const render = (label: string) =>
      renderToString(
        islandRoot(h(PerInstance, { label }), {
          identity: { component: "PerInstance", build: "b1" },
        }),
      );
    const a =
      '<div data-zfb-island="PerInstance" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{&quot;label&quot;:&quot;a&quot;}"><!--zr:1:0:c--><p><!--zr:1:1:t-->a!<!--/zr:1:1--></p><!--/zr:1:0--></div>';
    const b =
      '<div data-zfb-island="PerInstance" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{&quot;label&quot;:&quot;b&quot;}"><!--zr:1:0:c--><p><!--zr:1:1:t-->b!<!--/zr:1:1--></p><!--/zr:1:0--></div>';

    expect([render("a"), render("b"), render("a"), render("b")]).toEqual([a, b, a, b]);
  });

  it("escapes text and attribute bytes independently", () => {
    expect(renderToString(h("p", { title: 'A " & < >' }, "A & < >"))).toBe(
      '<p title="A &quot; &amp; &lt; &gt;">A &amp; &lt; &gt;</p>',
    );
  });

  it("uses SVG spellings, boolean presence, and stringified ARIA booleans", () => {
    const page = h(
      Fragment,
      null,
      h("svg", { viewBox: "0 0 1 1" }, h("path", { "stroke-width": "1.8" })),
      h("button", { disabled: true, hidden: false, "aria-pressed": false, "data-empty": "" }),
    );

    expect(renderToString(page)).toBe(
      '<svg viewBox="0 0 1 1"><path stroke-width="1.8"></path></svg><button disabled aria-pressed="false" data-empty=""></button>',
    );
  });

  it("does not activate callbacks or effects and drops every SSR subscription", async () => {
    const value = signal("server");
    let activationRuns = 0;
    let effectRuns = 0;
    let listenerRuns = 0;

    function ServerOnly() {
      const scope = getScope();
      scope.onActivate(() => {
        activationRuns++;
      });
      scope.effect(() => {
        effectRuns++;
        void value.value;
      });
      return h("button", { "on:click": () => listenerRuns++ }, value);
    }

    expect(typeof globalThis.document).toBe("undefined");
    expect(renderToString(h(ServerOnly, null))).toBe("<button>server</button>");
    expect(subscriberCount(value)).toBe(0);
    await flush();
    expect(activationRuns).toBe(0);
    expect(effectRuns).toBe(0);
    expect(listenerRuns).toBe(0);

    expect(
      renderToString(
        islandRoot(h(ServerOnly, null), {
          identity: { component: "ServerOnly", build: "b1" },
        }),
      ),
    ).toBe(
      '<div data-zfb-island="ServerOnly" data-when="load" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"><!--zr:1:0:c--><button><!--zr:1:1:t-->server<!--/zr:1:1--></button><!--/zr:1:0--></div>',
    );
    expect(subscriberCount(value)).toBe(0);
    await flush();
    expect(activationRuns).toBe(0);
    expect(effectRuns).toBe(0);
    expect(listenerRuns).toBe(0);
  });
});
