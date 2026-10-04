import { h, type Component, type Diagnostic, type IslandIdentity } from "./index.js";
import { hydrate as hydrateRoot, mount as mountRoot } from "./client.js";
import { flush as flushUpdates } from "./scheduler.js";
import { islandRoot, renderToString } from "./server.js";
import { parseProps } from "./props-transport.js";
import type { RootHandle } from "./root.js";

const testBuild = "test";

function componentName(component: Component<any>): string {
  const name = component.name;
  const displayName = (component as typeof component & { displayName?: string }).displayName;
  if (!name || (displayName && displayName !== name))
    throw new TypeError(`ZR_ISLAND_IDENTITY: invalid component name ${name || "<anonymous>"}`);
  return displayName ?? name;
}

export interface IslandTestOptions {
  /** The DOM document used to parse SSR markup and attach the host. */
  document: Document;
  /** Override the client identity to exercise identity failure cases explicitly. */
  identity?: IslandIdentity;
}

export interface IslandTest {
  readonly html: string;
  readonly host: Element;
  /** Change this before activation to test a mismatched client identity. */
  identity: IslandIdentity;
  readonly diagnostics: Diagnostic[];
  hydrate(): RootHandle | null;
  mount(): RootHandle | null;
  flush(): Promise<void>;
  dispose(): void;
}

/** Render one real island, parse it into a connected host, then activate it on demand. */
export function createIslandTest<P extends Record<string, unknown>>(
  component: Component<P>,
  props: P,
  options: IslandTestOptions,
): IslandTest {
  const serverIdentity = { component: componentName(component), build: testBuild };
  const html = renderToString(islandRoot(h(component, props), { identity: serverIdentity }));
  const owner = options.document.createElement("div");
  owner.innerHTML = html;
  const host = owner.firstElementChild;
  if (!host || owner.children.length !== 1)
    throw new TypeError("ZR_ISLAND_TEST: expected one island host");
  const islandHost = host;
  if (!options.document.body) throw new TypeError("ZR_ISLAND_TEST: document requires a body");
  options.document.body.append(owner);

  const diagnostics: Diagnostic[] = [];
  let root: RootHandle | null = null;
  let disposed = false;
  const test: IslandTest = {
    html,
    host: islandHost,
    identity: options.identity ?? serverIdentity,
    diagnostics,
    hydrate() {
      return activate(hydrateRoot);
    },
    mount() {
      return activate(mountRoot);
    },
    flush: flushUpdates,
    dispose() {
      if (disposed) return;
      disposed = true;
      try {
        root?.unmount();
      } finally {
        islandHost.remove();
        owner.remove();
      }
    },
  };
  function activate(run: typeof hydrateRoot): RootHandle | null {
    if (disposed) throw new Error("ZR_ISLAND_TEST: disposed test");
    // The client receives the serialized transport payload, not the original props object.
    let clientProps: Record<string, unknown>;
    try {
      clientProps = parseProps(islandHost.getAttribute("data-props") ?? "");
    } catch (error) {
      diagnostics.push({
        code: "ZR_PROPS",
        phase: "preflight",
        component: test.identity.component,
        componentStack: [test.identity.component],
        path: `/${islandHost.localName}`,
        expected: "valid serialized props",
        actual: String(error),
        protocol: "zudo-react/1",
        build: test.identity.build,
      });
      return null;
    }
    const result = run(h(component, clientProps), islandHost, {
      identity: test.identity,
      report: (diagnostic) => diagnostics.push(diagnostic),
    });
    if (result) root = result;
    return result;
  }
  return test;
}

export interface IslandTestContextOptions {
  components: Readonly<Record<string, Component<any>>> | readonly Component<any>[];
  build?: string;
}

/** Supply scanner metadata to synchronous full-page SDK Island rendering. */
export function withIslandTestContext<T>(options: IslandTestContextOptions, callback: () => T): T {
  const scope = globalThis as typeof globalThis & { __zfb?: Record<string, unknown> | undefined };
  const hadMetadata = Object.hasOwn(scope, "__zfb");
  const previous = scope.__zfb;
  const components = Array.isArray(options.components)
    ? options.components
    : Object.values(options.components);
  const names = [...new Set(components.map(componentName))];
  scope.__zfb = {
    ...previous,
    zudoReactBuild: options.build ?? testBuild,
    zudoReactIslands: names,
  };
  try {
    const result = callback();
    if (
      result !== null &&
      typeof result === "object" &&
      "then" in result &&
      typeof result.then === "function"
    )
      throw new TypeError("ZR_ISLAND_TEST: context callback must be synchronous");
    return result;
  } finally {
    if (hadMetadata) scope.__zfb = previous;
    else delete scope.__zfb;
  }
}
