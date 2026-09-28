import { isDescription, type Description } from "./zudo-react/index.js";
import { islandRoot } from "./zudo-react/server.js";
import type { VNode } from "./jsx-types.js";
import type { Child } from "./zudo-react/description.js";
import type { When } from "./types.js";

function singleChild(value: unknown): Description {
  if (Array.isArray(value)) {
    const children = value
      .flat(Infinity)
      .filter((child) => child != null && typeof child !== "boolean");
    if (children.length !== 1)
      throw new TypeError("ZR_ISLAND_CHILD: exactly one component child required");
    return singleChild(children[0]);
  }
  if (!isDescription(value) || typeof value.type !== "function")
    throw new TypeError("ZR_ISLAND_CHILD: exactly one function component child required");
  return value;
}

export function ownedIslandBoundary(
  child: VNode,
  fallback: VNode | undefined,
  when: When,
  media: string | undefined,
): Description {
  const description = singleChild(child);
  const component =
    (description.type as typeof description.type & { displayName?: string }).displayName ??
    (description.type as Function).name;
  if (!component) throw new TypeError("ZR_ISLAND_IDENTITY: anonymous component");
  const metadata = (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild?: string; zudoReactIslands?: readonly string[] };
    }
  ).__zfb;
  const build = metadata?.zudoReactBuild;
  if (!build) throw new TypeError(`ZR_ISLAND_IDENTITY: ${component} has no build identity`);
  if (metadata?.zudoReactIslands && !metadata.zudoReactIslands.includes(component))
    throw new TypeError(`ZR_ISLAND_IDENTITY: ${component} is not registered by the scanner`);
  return islandRoot(description, {
    identity: { component, build },
    when,
    media,
    skipSsr: fallback !== undefined,
    fallback: fallback as Child,
  });
}
