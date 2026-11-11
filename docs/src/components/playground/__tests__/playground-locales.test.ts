import { h, signal } from "@takazudo/zfb/zudo-react";
import { renderToString as render } from "@takazudo/zfb/zudo-react/server";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";

// The virtual route context stays on the server. Clients receive plain strings.
vi.mock("virtual:zudo-doc-route-context", () => ({
  routeContext: { settings: { base: "/wind-docs-preview/" } },
}));
vi.mock("../../../navigation-chrome", () => ({ createNavigationChrome: () => ({}) }));

import { chromeBindings } from "../../../chrome-bindings";
import CompilePlayground from "../compile-playground";
import DiagnosticsList from "../diagnostics-list";
import HighlightPlayground from "../highlight-playground";
import { localizedSamples, playgroundApiHref, playgroundCopy } from "../host-copy";
import ParsePlayground from "../parse-playground";
import PlaygroundShell from "../playground-shell";
import RenderPlayground from "../render-playground";

const tools = [
  ["RenderPlayground", RenderPlayground],
  ["CompilePlayground", CompilePlayground],
  ["ParsePlayground", ParsePlayground],
  ["HighlightPlayground", HighlightPlayground],
] as const;

afterEach(() => vi.unstubAllGlobals());

describe("API playground locales", () => {
  it.each(tools)(
    "defaults %s to English and renders Japanese from serialized props",
    (_name, Component) => {
      const english = render(h(Component, {}));
      expect(english).toContain(">Run</button>");
      expect(english).toContain('href="/docs/api/md-wasm/"');
      const props = JSON.parse(JSON.stringify({ locale: "ja", basePath: "/wind-docs-preview/" }));
      const japanese = render(h(Component, props));
      expect(japanese).toContain(">実行</button>");
      expect(japanese).toContain("初回の実行時に WebAssembly モジュールをダウンロードします。");
      expect(japanese).toContain('href="/wind-docs-preview/ja/docs/api/md-wasm/"');
      expect(japanese).toContain(">サンプル</legend>");
      expect(japanese).not.toContain(">Run</button>");
      expect(japanese).not.toContain("The first Run downloads");
    },
  );

  it.each(tools)(
    "forwards %s locale and configured base through the fixed Island wrapper",
    (name, Component) => {
      vi.stubGlobal("__zfb", {
        zudoReactBuild: "locale-test",
        zudoReactIslands: tools.map(([key]) => key),
      });
      const Wrapper = chromeBindings.mdxExtras![name];
      const html = render(h(Wrapper, { locale: "ja" }));
      expect(html).toContain(`data-zfb-island="${name}"`);
      const encoded = html.match(/data-props="([^"]+)"/)?.[1];
      expect(encoded).toBeDefined();
      const props = JSON.parse(encoded!.replaceAll("&quot;", '"'));
      expect(props).toEqual({ locale: "ja", basePath: "/wind-docs-preview/" });
      // Render the very payload used for hydration, without wrapper/context.
      const transportedHtml = render(h(Component, props));
      expect(transportedHtml).toContain('href="/wind-docs-preview/ja/docs/api/md-wasm/"');
      expect(transportedHtml).toContain(">実行</button>");
    },
  );

  it.each(tools)("defaults the %s wrapper transport to English", (name) => {
    vi.stubGlobal("__zfb", {
      zudoReactBuild: "locale-test",
      zudoReactIslands: tools.map(([key]) => key),
    });
    const html = render(h(chromeBindings.mdxExtras![name], {}));
    const encoded = html.match(/data-props="([^"]+)"/)?.[1];
    expect(encoded).toBeDefined();
    expect(JSON.parse(encoded!.replaceAll("&quot;", '"'))).toEqual({
      locale: "en",
      basePath: "/wind-docs-preview/",
    });
    expect(html).toContain('href="/wind-docs-preview/docs/api/md-wasm/"');
    expect(html.replace(/<!--[\s\S]*?-->/g, "")).toContain(">Run</button>");
  });

  it("retains API keys, sample IDs and executable source when localizing labels", () => {
    const samples = [
      {
        id: "unknown-language",
        label: "Unknown language",
        value: "<tag>&",
        language: "not-a-bundled-syntax",
      },
    ];
    expect(localizedSamples(samples, "ja")).toEqual([{ ...samples[0], label: "未知の言語" }]);
    expect(samples[0].label).toBe("Unknown language");
    const html = render(h(RenderPlayground, { locale: "ja" }));
    expect(html).toContain("pipeline.gfm.strikethrough");
    expect(html).toContain('pipeline.features.directives.note = "Note"');
    expect(html).toContain("hello from zfb-md-wasm");
    expect(html).toContain("注記ディレクティブ");
    expect(html).toContain('title="レンダリングされた HTML のプレビュー"');
  });

  it("localizes pending controls and accessible output labels", () => {
    const html = render(
      h(PlaygroundShell, {
        value: signal("source"),
        onRun: () => {},
        pending: signal(true),
        hasRun: signal(true),
        labels: playgroundCopy("ja"),
        textareaLabel: playgroundCopy("ja").markdownSource,
      }),
    );
    expect(html).toContain(">実行中…</button>");
    expect(html).toContain('aria-label="出力"');
    expect(html).toContain('aria-label="Markdown または MDX のソース"');
    expect(html).not.toContain("初回の実行時");
  });

  it("localizes diagnostic chrome and location while retaining engine messages and source IDs", () => {
    const html = render(
      h(DiagnosticsList, {
        locale: "ja",
        diagnostics: signal([
          {
            severity: "warning" as const,
            source: "highlight",
            message: "Unknown syntax: original engine text",
            line: 2,
            column: 3,
          },
        ]),
      }),
    );
    expect(html).toContain('aria-label="診断"');
    expect(html).toContain(">警告</strong>");
    expect(html).toContain("2 行、3 列");
    expect(html).toContain("Unknown syntax: original engine text");
    expect(html).toContain(">highlight</span>");
    const empty = render(
      h(DiagnosticsList, {
        locale: "ja",
        diagnostics: signal([]),
        emptyMessage: signal(playgroundCopy("ja").noDiagnostics),
      }),
    );
    expect(empty).toContain("診断はありません。");
  });

  it.each(["", "/", "/wind-docs-preview", "/wind-docs-preview/", "https://example.com/docs-host/"])(
    "preserves configured base %s in both locales",
    (base) => {
      const prefix = base.replace(/\/$/, "");
      expect(playgroundApiHref("en", base)).toBe(`${prefix}/docs/api/md-wasm/`);
      expect(playgroundApiHref("ja", base)).toBe(`${prefix}/ja/docs/api/md-wasm/`);
    },
  );
});
