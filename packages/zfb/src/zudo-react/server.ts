import type { Child, Description } from "./description.js";
import type { IslandIdentity } from "./index.js";
import { createDescription } from "./description.js";
import { islandRootType } from "./island-root-type.js";
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
export function islandRoot(child: Description, options: IslandOptions): Description {
  return createDescription(islandRootType as never, { child, options });
}
export function renderToString(node: Child, options: RenderOptions = {}): string {
  return renderHtml(node, options);
}
