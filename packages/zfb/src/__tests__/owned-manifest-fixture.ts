import {
  mountIslands as mountOwnedIslands,
  mountNewIslands as mountNewOwnedIslands,
  type IslandManifest,
} from "../runtime.js";
import type { RootHandle } from "../zudo-react/root.js";

type TestEntry = {
  identity?: { component: string; build: string };
  mount: (props: Record<string, unknown>, element: Element, mode: "hydrate" | "render") => unknown;
  dispose?: (element: Element) => void;
};
type TestManifest = Record<string, TestEntry>;

let current: TestManifest = {};

function markWrappers(manifest: TestManifest): void {
  for (const element of document.querySelectorAll<HTMLElement>(
    "[data-zfb-island],[data-zfb-island-skip-ssr]",
  )) {
    const name =
      element.getAttribute("data-zfb-island") ?? element.getAttribute("data-zfb-island-skip-ssr");
    const entry = name ? manifest[name] : undefined;
    if (!entry) continue;
    element.setAttribute(
      "data-zfb-transport",
      element.getAttribute("data-zfb-transport") ?? "json/1",
    );
    element.setAttribute(
      "data-zfb-protocol",
      element.getAttribute("data-zfb-protocol") ?? "zudo-react/1",
    );
    element.setAttribute(
      "data-zfb-build",
      element.getAttribute("data-zfb-build") ?? entry.identity?.build ?? "test",
    );
    element.setAttribute("data-props", element.getAttribute("data-props") ?? "{}");
  }
}

export function mountTestIslands(
  manifest: TestManifest,
  mount: (owned: IslandManifest) => void = mountOwnedIslands,
): void {
  current = manifest;
  markWrappers(manifest);
  const owned: Record<string, IslandManifest[string]> = {};
  for (const [name, entry] of Object.entries(manifest)) {
    const identity = entry.identity ?? { component: name, build: "test" };
    owned[name] = {
      identity,
      mount(props, element, mode) {
        const result = entry.mount(props, element, mode);
        if (result === null) return null;
        const returned = result as Partial<RootHandle> | undefined;
        let disposed = false;
        return {
          protocol: "zudo-react/1",
          identity,
          get disposed() {
            return disposed;
          },
          dispose() {
            disposed = true;
            if (returned && typeof returned.dispose === "function") returned.dispose();
            else entry.dispose?.(element);
          },
          unmount() {
            returned?.unmount?.();
          },
        };
      },
    };
  }
  mount(owned);
}

export function mountNewTestIslands(): void {
  markWrappers(current);
  mountNewOwnedIslands();
}
