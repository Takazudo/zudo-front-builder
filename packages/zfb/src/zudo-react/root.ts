import type { Diagnostic, IslandIdentity, Reporter } from "./index.js";
import type { RuntimeScope } from "./scope.js";

export interface RootOptions {
  identity: IslandIdentity;
  signal?: AbortSignal | undefined;
  report?: Reporter | undefined;
}
export interface RootHandle {
  readonly protocol: "zudo-react/1";
  readonly identity: IslandIdentity;
  readonly disposed: boolean;
  dispose(): void;
  unmount(): void;
}
export const ROOT_KEY = Symbol.for("@takazudo/zfb/zudo-react/root-v1");
type RootElement = Element & { [ROOT_KEY]?: RootHandle };
export function storedRoot(container: Element): RootHandle | undefined {
  return (container as RootElement)[ROOT_KEY];
}
export function report(options: RootOptions, diagnostic: Diagnostic): void {
  try {
    (options.report ?? ((value) => console.error(value)))(diagnostic);
  } catch (error) {
    console.error(error);
  }
}
export function diagnostic(
  options: RootOptions,
  code: string,
  phase: Diagnostic["phase"],
  path: string,
  expected: string,
  actual: string,
  stack: readonly string[] = [options.identity.component],
): Diagnostic {
  return {
    code,
    phase,
    component: stack.at(-1) ?? options.identity.component,
    componentStack: stack,
    path,
    expected,
    actual,
    protocol: "zudo-react/1",
    build: options.identity.build,
  };
}
export function rootPath(element: Element): string {
  const parts: string[] = [];
  for (let node: Element | null = element; node; node = node.parentElement) {
    const index = node.parentElement
      ? Array.prototype.indexOf.call(node.parentElement.children, node) + 1
      : 1;
    parts.unshift(`${node.localName}:nth-child(${index})`);
  }
  return `/${parts.join("/")}`;
}
export function createRoot(
  container: Element,
  options: RootOptions,
  scope: RuntimeScope,
  cleanups: Array<() => void>,
  owned: readonly Node[],
): RootHandle {
  let disposed = false;
  let removed = false;
  const handle: RootHandle = {
    protocol: "zudo-react/1",
    identity: options.identity,
    get disposed() {
      return disposed;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const cleanup of [...cleanups].reverse()) {
        try {
          cleanup();
        } catch (error) {
          report(
            options,
            diagnostic(
              options,
              "ZR_CLEANUP_ERROR",
              "cleanup",
              rootPath(container),
              "cleanup",
              String(error),
            ),
          );
        }
      }
      scope.dispose();
    },
    unmount() {
      handle.dispose();
      if (removed || storedRoot(container) !== handle) return;
      removed = true;
      for (const node of owned) if (node.parentNode === container) container.removeChild(node);
      if (storedRoot(container) === handle) delete (container as RootElement)[ROOT_KEY];
    },
  };
  (container as RootElement)[ROOT_KEY] = handle;
  return handle;
}
