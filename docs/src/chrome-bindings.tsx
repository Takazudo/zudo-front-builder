/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import PlaygroundProbe from "./components/playground-probe";

// Island derives the hydration-marker name from type.displayName ?? type.name.
// Pin it so minification cannot create a marker/registry mismatch that would
// otherwise leave this island dead without failing the build.
(PlaygroundProbe as { displayName?: string }).displayName = "PlaygroundProbe";

const PlaygroundProbeIsland = () => Island({ when: "visible", children: <PlaygroundProbe /> });

export const chromeBindings = defineChromeBindings({
  mdxExtras: { PlaygroundProbe: PlaygroundProbeIsland },
});
