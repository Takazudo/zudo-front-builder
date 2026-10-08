/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import { createNavigationChrome } from "./navigation-chrome";
import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import DesignSystemPlayground from "./components/playground/design-system-playground";
import CompilePlayground from "./components/playground/compile-playground";
import HighlightPlayground from "./components/playground/highlight-playground";
import ParsePlayground from "./components/playground/parse-playground";
import RenderPlayground from "./components/playground/render-playground";
import WindPreviewEnhancer from "./components/wind-preview-enhancer";

const DesignSystemPlaygroundIsland = () =>
  Island({ when: "visible", children: <DesignSystemPlayground /> });

const RenderPlaygroundIsland = () => Island({ when: "visible", children: <RenderPlayground /> });
const CompilePlaygroundIsland = () => Island({ when: "visible", children: <CompilePlayground /> });
const ParsePlaygroundIsland = () => Island({ when: "visible", children: <ParsePlayground /> });
const HighlightPlaygroundIsland = () =>
  Island({ when: "visible", children: <HighlightPlayground /> });

const linkClass = "text-fg underline hover:text-accent";

const HomeExtras = ({ locale }: { locale: string }) => {
  const label = locale === "ja" ? "zfb で作られたもの: " : "Built on zfb: ";
  const zudoDocTitle = locale === "ja" ? "ドキュメントフレームワーク" : "documentation framework";
  const ccResDocTitle = locale === "ja" ? "デスクトップアプリ" : "desktop app";
  return (
    <span>
      <span>{label}</span>
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

const baseBindings = defineChromeBindings({
  mdxExtras: {
    DesignSystemPlayground: DesignSystemPlaygroundIsland,
    RenderPlayground: RenderPlaygroundIsland,
    CompilePlayground: CompilePlaygroundIsland,
    ParsePlayground: ParsePlaygroundIsland,
    HighlightPlayground: HighlightPlaygroundIsland,
    WindPreviewEnhancer,
  },
  homeExtras: HomeExtras,
});

export const chromeBindings = {
  ...baseBindings,
  ...defineChromeBindings(createNavigationChrome(baseBindings)),
};
