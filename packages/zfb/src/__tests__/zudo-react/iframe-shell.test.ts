import { beforeEach, describe, expect, it, vi } from "vitest";
import { flush, h, signal } from "../../zudo-react/index.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Frame", build: "b1" };
const shell = {
  src: "/embedded",
  srcdoc: "<p>trusted</p>",
  sandbox: "allow-scripts",
  allow: "fullscreen",
};
let currentNode: ReturnType<typeof h>;
function Frame() {
  return currentNode;
}
function clientRoot(node: ReturnType<typeof h>) {
  currentNode = node;
  return h(Frame);
}

function hostFor(node: ReturnType<typeof h>, skipSsr = false): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(clientRoot(node), { identity, skipSsr }));
  return host.firstElementChild!;
}

beforeEach(() => document.body.replaceChildren());

describe("childless iframe shell", () => {
  it("renders static and island shells with escaped srcdoc", () => {
    const frame = h("iframe", shell);
    const html =
      '<iframe src="/embedded" srcdoc="&lt;p&gt;trusted&lt;/p&gt;" sandbox="allow-scripts" allow="fullscreen"></iframe>';
    expect(renderToString(frame)).toBe(html);
    expect(renderToString(islandRoot(clientRoot(h("div", null, frame)), { identity }))).toContain(
      html,
    );
    expect(
      renderToString(islandRoot(clientRoot(h("div", null, frame)), { identity, skipSsr: true })),
    ).not.toContain(html);
  });

  it.each([undefined, null, false, true, "", [], [null, false, "", []]])(
    "accepts non-rendering children %j",
    (children) => {
      const frame = h("iframe", { ...shell, children } as never);
      expect(renderToString(frame)).toContain("</iframe>");
      const host = hostFor(h("div", null, frame));
      const diagnostics: Diagnostic[] = [];
      expect(
        hydrate(clientRoot(h("div", null, frame)), host, {
          identity,
          report: (item) => diagnostics.push(item),
        }),
      ).not.toBeNull();
      expect(diagnostics).toEqual([]);
      expect(host.querySelector("iframe")?.childNodes).toHaveLength(0);
    },
  );

  it.each(["fallback", 0, [null, "fallback"], h("span", null, "fallback"), signal("")])(
    "rejects renderable children %j",
    (children) => {
      const frame = h("iframe", { ...shell, children } as never);
      expect(() => renderToString(frame)).toThrow("ZR_PARSER_CONTEXT");
      expect(() =>
        renderToString(islandRoot(clientRoot(h("div", null, frame)), { identity })),
      ).toThrow("ZR_PARSER_CONTEXT");
      const diagnostics: Diagnostic[] = [];
      const host = hostFor(h("div"), true);
      expect(
        mount(clientRoot(h("div", null, frame)), host, {
          identity,
          report: (item) => diagnostics.push(item),
        }),
      ).toBeNull();
      expect(diagnostics.at(-1)?.code).toBe("ZR_UNSUPPORTED_POSITION");
    },
  );

  it.each([undefined, null, "", "<b>fallback</b>"])("rejects rawHtml even when %j", (rawHtml) => {
    const frame = h("iframe", { ...shell, rawHtml } as never);
    expect(() => renderToString(frame)).toThrow("ZR_RAW_HTML");
    const diagnostics: Diagnostic[] = [];
    const host = hostFor(h("div"), true);
    expect(
      mount(clientRoot(h("div", null, frame)), host, {
        identity,
        report: (item) => diagnostics.push(item),
      }),
    ).toBeNull();
    expect(diagnostics.at(-1)?.code).toBe("ZR_RAW_HTML");
  });

  it.each(["template", "noscript", "xmp", "noembed", "noframes", "plaintext"])(
    "keeps %s sensitive in islands",
    (tag) => {
      expect(() =>
        renderToString(islandRoot(clientRoot(h("div", null, h(tag))), { identity })),
      ).toThrow("ZR_PARSER_CONTEXT");
    },
  );

  it("rejects unexpected fallback DOM during hydration", () => {
    const node = h("div", null, h("iframe", shell));
    const host = hostFor(node);
    const frame = host.querySelector("iframe")!;
    frame.append("fallback");
    const diagnostics: Diagnostic[] = [];
    expect(
      hydrate(clientRoot(node), host, {
        identity,
        report: (item) => diagnostics.push(item),
      }),
    ).toBeNull();
    expect(diagnostics.at(-1)?.code).toBe("ZR_HYDRATION_MISMATCH");
    expect(frame.textContent).toBe("fallback");
  });

  it.each(["hydrate", "mount"] as const)(
    "owns attributes, ref, and load lifecycle on %s",
    async (mode) => {
      const src = signal("/first");
      const srcdoc = signal("<p>first</p>");
      const sandbox = signal("allow-scripts");
      const allow = signal("fullscreen");
      const ref = { current: null as HTMLIFrameElement | null };
      const onLoad = vi.fn();
      const frame = h("iframe", { src, srcdoc, sandbox, allow, ref, "on:load": onLoad });
      const node = h("div", null, frame);
      const host = hostFor(node, mode === "mount");
      const existing = host.querySelector("iframe");
      const diagnostics: Diagnostic[] = [];
      const handle = (mode === "hydrate" ? hydrate : mount)(clientRoot(node), host, {
        identity,
        report: (item) => diagnostics.push(item),
      });
      expect(handle).not.toBeNull();
      const owned = host.querySelector("iframe")!;
      expect(mode === "hydrate" ? owned === existing : owned !== existing).toBe(true);
      expect(ref.current).toBe(owned);
      owned.dispatchEvent(new Event("load"));
      expect(onLoad).toHaveBeenCalledTimes(1);
      src.value = "/second";
      srcdoc.value = "<p>second</p>";
      sandbox.value = "allow-forms";
      allow.value = "camera";
      await flush();
      expect(owned.getAttribute("src")).toBe("/second");
      expect(owned.getAttribute("srcdoc")).toBe("<p>second</p>");
      expect(owned.getAttribute("sandbox")).toBe("allow-forms");
      expect(owned.getAttribute("allow")).toBe("camera");
      expect(owned.childNodes).toHaveLength(0);
      expect(diagnostics).toEqual([]);
      handle?.dispose();
      expect(ref.current).toBeNull();
      owned.dispatchEvent(new Event("load"));
      expect(onLoad).toHaveBeenCalledTimes(1);
      handle?.unmount();
      expect(host.querySelector("iframe")).toBeNull();
    },
  );
});
