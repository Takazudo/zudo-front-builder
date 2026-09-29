import type { Child, Description } from "./description.js";
import type { IslandIdentity } from "./index.js";
import { createDescription } from "./description.js";
import { renderHtml } from "./render-html.js";

export { serializeProps } from "./props-transport.js";

export interface RenderOptions {
  island?: IslandIdentity | undefined;
}
export interface IslandOptions {
  identity: IslandIdentity;
  when?: "load" | "idle" | "visible" | "media" | undefined;
  media?: string | undefined;
  skipSsr?: boolean | undefined;
  fallback?: Child;
}
export const islandRootType = Symbol.for("@takazudo/zfb/zudo-react/island-root-v1");
export function islandRoot(child: Description, options: IslandOptions): Description {
  return createDescription(islandRootType as never, { child, options });
}
export function renderToString(node: Child, options: RenderOptions = {}): string {
  return renderHtml(node, options);
}
