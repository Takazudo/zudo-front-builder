import type { VNode } from "./jsx-types.js";
import type { When } from "./types.js";

// The default framework retains the legacy wrapper path in island.ts.
export function ownedIslandBoundary(
  _child: VNode,
  _fallback: VNode | undefined,
  _when: When,
  _media: string | undefined,
): undefined {
  return undefined;
}
