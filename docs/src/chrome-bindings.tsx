/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import PlaygroundProbe from "./components/playground-probe";
import CompilePlayground from "./components/playground/compile-playground";
import HighlightPlayground from "./components/playground/highlight-playground";
import ParsePlayground from "./components/playground/parse-playground";
import RenderPlayground from "./components/playground/render-playground";

// Island derives the hydration-marker name from type.displayName ?? type.name.
// Pin it so minification cannot create a marker/registry mismatch that would
// otherwise leave this island dead without failing the build.
(PlaygroundProbe as { displayName?: string }).displayName = "PlaygroundProbe";

const PlaygroundProbeIsland = () => Island({ when: "visible", children: <PlaygroundProbe /> });
const RenderPlaygroundIsland = () => Island({ when: "visible", children: <RenderPlayground /> });
const CompilePlaygroundIsland = () => Island({ when: "visible", children: <CompilePlayground /> });
const ParsePlaygroundIsland = () => Island({ when: "visible", children: <ParsePlayground /> });
const HighlightPlaygroundIsland = () =>
  Island({ when: "visible", children: <HighlightPlayground /> });

export const chromeBindings = defineChromeBindings({
  mdxExtras: {
    PlaygroundProbe: PlaygroundProbeIsland,
    RenderPlayground: RenderPlaygroundIsland,
    CompilePlayground: CompilePlaygroundIsland,
    ParsePlayground: ParsePlaygroundIsland,
    HighlightPlayground: HighlightPlaygroundIsland,
  },
});
