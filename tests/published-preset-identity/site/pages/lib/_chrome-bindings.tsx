// Host bindings reach the package-owned doc route through
// `virtual:zudo-doc-chrome-bindings`.
import type { ChromeHostBindings } from "@takazudo/zudo-doc/factory-context";
import { PresetProbeBoundary } from "../../src/components/preset-probe-boundary";

export const chromeBindings: ChromeHostBindings = {
  mdxExtras: { PresetProbe: PresetProbeBoundary },
};
