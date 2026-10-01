import { isDescription, type Description } from "./zudo-react/index.js";
import { islandRoot } from "./zudo-react/server.js";
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
  child: Description | undefined,
  fallback: Child | undefined,
  when: When,
  media: string | undefined,
): Description {
  const description = singleChild(child);
  const functionName = (description.type as Function).name;
  const displayName = (description.type as typeof description.type & { displayName?: string })
    .displayName;
  if (!functionName) throw new TypeError("ZR_ISLAND_IDENTITY: anonymous component");
  if (displayName && displayName !== functionName)
    throw new TypeError(
      `ZR_ISLAND_IDENTITY: ${functionName} conflicts with displayName ${displayName}`,
    );
  const component = displayName ?? functionName;
  const metadata = (
    globalThis as typeof globalThis & {
      __zfb?: { zudoReactBuild?: string; zudoReactIslands?: readonly string[] };
    }
  ).__zfb;
  const build = metadata?.zudoReactBuild;
  if (!build) throw new TypeError(`ZR_ISLAND_IDENTITY: ${component} has no build identity`);
  if (!metadata?.zudoReactIslands)
    throw new TypeError(`ZR_ISLAND_IDENTITY: ${component} has no scanner identity metadata`);
  if (!metadata.zudoReactIslands.includes(component))
    throw new TypeError(`ZR_ISLAND_IDENTITY: ${component} is not registered by the scanner`);
  return islandRoot(description, {
    identity: { component, build },
    when,
    media,
    skipSsr: fallback !== undefined,
    fallback,
  });
}
