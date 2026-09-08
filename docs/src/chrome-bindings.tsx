/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import CompilePlayground from "./components/playground/compile-playground";
import HighlightPlayground from "./components/playground/highlight-playground";
import ParsePlayground from "./components/playground/parse-playground";
import RenderPlayground from "./components/playground/render-playground";

const RenderPlaygroundIsland = () => Island({ when: "visible", children: <RenderPlayground /> });
const CompilePlaygroundIsland = () => Island({ when: "visible", children: <CompilePlayground /> });
const ParsePlaygroundIsland = () => Island({ when: "visible", children: <ParsePlayground /> });
const HighlightPlaygroundIsland = () =>
  Island({ when: "visible", children: <HighlightPlayground /> });

// Verified still necessary with @takazudo/zudo-doc@5.19.1 (issue #2960): the
// non-wrapping hero flex row squeezes its items on narrow viewports. Keep both
// whitespace-nowrap on links and hidden sm:inline on the explanatory label.
const linkClass = "text-fg underline hover:text-accent whitespace-nowrap";

const HomeExtras = ({ locale }: { locale: string }) => {
  const label = locale === "ja" ? "zfb で作られたもの: " : "Built on zfb: ";
  const zudoDocTitle = locale === "ja" ? "ドキュメントフレームワーク" : "documentation framework";
  const ccResDocTitle = locale === "ja" ? "デスクトップアプリ" : "desktop app";
  return (
    <span>
      <span class="hidden sm:inline">{label}</span>
      <a href="https://github.com/zudolab/zudo-doc" title={zudoDocTitle} class={linkClass}>
        zudo-doc
      </a>
      {" · "}
      <a href="https://github.com/Takazudo/ccresdoc" title={ccResDocTitle} class={linkClass}>
        CCResDoc
      </a>
    </span>
  );
};

export const chromeBindings = defineChromeBindings({
  mdxExtras: {
    RenderPlayground: RenderPlaygroundIsland,
    CompilePlayground: CompilePlaygroundIsland,
    ParsePlayground: ParsePlaygroundIsland,
    HighlightPlayground: HighlightPlaygroundIsland,
  },
  homeExtras: HomeExtras,
});
