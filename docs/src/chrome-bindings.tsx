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

export const chromeBindings = defineChromeBindings({
  mdxExtras: {
    RenderPlayground: RenderPlaygroundIsland,
    CompilePlayground: CompilePlaygroundIsland,
    ParsePlayground: ParsePlaygroundIsland,
    HighlightPlayground: HighlightPlaygroundIsland,
  },
});
