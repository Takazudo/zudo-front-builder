import { describe, expect, it } from "vitest";
import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";

import { createPageRouter, type PageModule } from "../router.js";

it("renders a branded owned description through the server entry", async () => {
  const router = createPageRouter({
    pages: [
      {
        route: "/",
        module: async () => ({
          default: () => jsx("main", { children: jsx("strong", { children: "owned" }) }),
        }),
      },
    ],
    contentSnapshot: { collections: {} },
  });
  const response = await router(new Request("https://example.test/"));
  expect(response.status).toBe(200);
  expect(await response.text()).toContain("<main><strong>owned</strong></main>");
});

describe("structured render diagnostics", () => {
  const prefix = "[zfb-render-diagnostic] ";
  const diagnosticLine = (body: string) => {
    const line = body.split("\n").find((entry) => entry.startsWith(prefix));
    return line && JSON.parse(line.slice(prefix.length));
  };
  async function renderBody(page: PageModule, includeErrorStack: boolean): Promise<string> {
    const router = createPageRouter({
      pages: [{ route: "/", module: async () => page }],
      contentSnapshot: { collections: {} },
      includeErrorStack,
    });
    const response = await router(new Request("https://example.test/"));
    expect(response.status).toBe(500);
    return response.text();
  }
  function Field() {
    return jsx("input", { name: "q", autoComplete: "off", rawHtml: "<b>secret</b>" });
  }
  const page: PageModule = { default: () => jsx("form", { children: jsx(Field, {}) }) };

  it("replays a failed render to attach the failing element's generated jsx() call site", async () => {
    const body = await renderBody(page, true);
    const diagnostic = diagnosticLine(body);
    expect(diagnostic).toEqual({
      code: "ZR_PROP_DIALECT",
      path: "root",
      component: "static render",
      spelling: { name: "autoComplete", suggestion: "autocomplete" },
      site: {
        kind: "generated",
        specifier: expect.stringMatching(/router-owned-render\.test\.ts$/),
        line: 40,
        column: expect.any(Number),
      },
    });
    expect(body).not.toContain("secret");
    expect(Object.getOwnPropertySymbols(globalThis)).not.toContain(
      Symbol.for("@takazudo/zfb/zudo-react/site-capture-v1"),
    );
  });

  it("emits no structured line or call site outside the debug host", async () => {
    expect(await renderBody(page, false)).toBe(
      '[zfb-runtime] render threw for "/": ZR_PROP_DIALECT: input.autoComplete (use `autocomplete` instead of `autoComplete`) at root in static render',
    );
  });

  it("keeps the structural diagnostic when the replay does not fail identically", async () => {
    let renders = 0;
    const flaky: PageModule = {
      default: () => (renders++ === 0 ? jsx("input", { autoComplete: "off" }) : "ok"),
    };
    expect(diagnosticLine(await renderBody(flaky, true))).toEqual({
      code: "ZR_PROP_DIALECT",
      path: "root",
      component: "static render",
      spelling: { name: "autoComplete", suggestion: "autocomplete" },
    });
  });

  it("does not emit a structured line for ordinary errors", async () => {
    const body = await renderBody(
      {
        default: () => {
          throw new Error("boom");
        },
      },
      true,
    );
    expect(diagnosticLine(body)).toBeUndefined();
  });
});
