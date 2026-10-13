import type { Child, Description } from "./description.js";
import type { BoundaryHost, BuildContext, Owned } from "./hydrate.js";
import { boundaryProps } from "./error-boundary-type.js";
import type { Job } from "./scheduler.js";

// Each attempt has its own collection lifetime. Failed descriptions, markers,
// provisional forms and operations cannot remain reachable through the root.
export function renderErrorBoundary(
  value: Description,
  parent: Node,
  context: BuildContext,
  namespace: string,
  parentTag: string,
  path: string,
  host: BoundaryHost,
): void {
  // Use the owning renderer's scope/reactivity/form state, including when the
  // boundary function came from another package copy with the shared type token.
  const {
    owned,
    installOwned,
    selfMap,
    marker,
    range,
    fail,
    openMarker,
    matchingClose,
    recoverable,
    prepareForms,
    read,
    subscribe,
    untracked,
    withScope,
    enqueue,
  } = host;
  const props = boundaryProps(value);
  const owner = context.scope.child("ErrorBoundary");
  const prefix = `${context.prefix ?? ""}${context.next}.`;
  let current: Owned;
  let installed = false;
  let requested = false;
  let start: Node;
  let end: Node;
  let lastKey = props.resetKey && read(props.resetKey);
  const clear = (): void => {
    for (const node of range(start, end).slice(1, -1)) node.parentNode!.removeChild(node);
  };
  const attempt = (factory: () => Child, initial: boolean): Owned => {
    const local: BuildContext = {
      ...context,
      scope: owner,
      next: 0,
      prefix,
      operations: [],
      forms: [],
      formOwners: new WeakMap(),
      opaque: new WeakSet(),
      textSlots: new WeakSet(),
      displayOnlyTextareas: new WeakSet(),
      dynamicRanges: [],
      collectRootRanges: false,
      errorRanges: new WeakMap(),
      replacements: [],
    };
    const instance = owned(
      local,
      "ErrorBoundary attempt",
      namespace,
      parentTag,
      path,
      factory,
      true,
    );
    if (initial) {
      context.forms.push(...instance.forms);
      for (const form of instance.forms)
        context.formOwners.set(form, local.formOwners.get(form) ?? instance);
    }
    local.forms.length = 0;
    return instance;
  };
  const fallback = (error: unknown, initial: boolean): Owned => {
    if (!recoverable(error)) throw error;
    return attempt(() => props.fallback(error, reset), initial);
  };
  const prepare = (initial: boolean): Owned => {
    let replacement: Owned;
    try {
      replacement = attempt(props.children, initial);
    } catch (error) {
      return fallback(error, initial);
    }
    replacement.scope.renderRecovery = (error) => {
      // Disposal makes all work from this attempt inert in the current pass.
      current.scope.dispose();
      const next = fallback(error, false);
      return () => commit(next);
    };
    return replacement;
  };
  const commit = (replacement: Owned): void => {
    const map = selfMap(replacement.fragment);
    try {
      prepareForms(
        replacement.forms,
        map,
        "mount",
        context.container,
        [current.scope],
        start.parentNode,
      );
    } catch (error) {
      replacement.scope.dispose();
      throw error;
    }
    current.scope.dispose();
    clear();
    end.parentNode!.insertBefore(replacement.fragment, end);
    current = replacement;
    installOwned(current, map, context, "mount");
    if (owner.isActivated) current.scope.activate();
  };
  const job: Job = {
    id: `ErrorBoundary:${owner.id}`,
    phase: "commit",
    active: () => owner.active && installed,
    run: () => {
      requested = false;
      if (props.resetKey) lastKey = read(props.resetKey);
      let replacement: Owned;
      try {
        replacement = untracked(() => prepare(false));
      } catch (error) {
        const recover = owner.recoverRender(error);
        if (!recover) throw error;
        recover();
        return;
      }
      commit(replacement);
    },
    report: (code, actual) => owner.report(code, "update", actual),
  };
  function reset(): void {
    if (requested || !job.active()) return;
    requested = true;
    enqueue(job);
  }
  try {
    const ends = marker(context, parent, "e", () => {
      current = untracked(() => prepare(true));
      parent.appendChild(current.fragment);
    });
    context.errorRanges.set(ends[0], (wants, has, i, j, map, childPath) => {
      const wanted = wants[i]!;
      const found = has[j]!;
      const wantedEnd = matchingClose(wants, i);
      const foundEnd = matchingClose(has, j);
      if (wantedEnd < 0 || foundEnd < 0)
        fail(
          "ZR_HYDRATION_MISMATCH",
          "preflight",
          "balanced boundary",
          "missing marker",
          childPath,
        );
      validateBoundaryMarkers(
        has.slice(j + 1, foundEnd),
        `${openMarker(found)![1]}.`,
        childPath,
        host,
      );
      const close = has[foundEnd]!;
      map.set(wants[wantedEnd]!, close);
      const mapFresh = (node: Node): void => {
        map.set(node, node);
        for (const child of node.childNodes) mapFresh(child);
      };
      // Include whitespace nodes too: significant() is only a comparison view.
      const allFresh = range(wanted, wants[wantedEnd]!).slice(1, -1);
      for (const node of allFresh) mapFresh(node);
      context.replacements.push(() => {
        for (const node of range(found, close).slice(1, -1)) node.parentNode!.removeChild(node);
        for (const node of allFresh) close.parentNode!.insertBefore(node, close);
      });
      return [wantedEnd + 1, foundEnd + 1];
    });
    context.operations.push((map, cleanups) => {
      start = map.get(ends[0])!;
      end = map.get(ends[1])!;
      installOwned(current, map, context, "hydrate");
      installed = true;
      if (props.resetKey) {
        const key = props.resetKey;
        const subscription = subscribe(
          () => {
            const next = key.value;
            if (Object.is(lastKey, next)) return;
            lastKey = next;
            reset();
          },
          { active: () => owner.active, report: job.report },
        );
        subscription.run();
        withScope(owner, () => owner.onCleanup(() => subscription.dispose()));
      }
    });
  } catch (error) {
    owner.dispose();
    throw error;
  }
}

function validateBoundaryMarkers(
  nodes: Node[],
  prefix: string,
  path: string,
  host: BoundaryHost,
): void {
  const { fail, openMarker } = host;
  const seen = new Set<string>();
  const invalid = (): never =>
    fail("ZR_HYDRATION_MISMATCH", "preflight", "balanced boundary markers", "invalid marker", path);
  const visit = (children: Node[], namespace: string): void => {
    const stack: Array<{ id: string; namespace: string }> = [];
    for (const node of children) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        visit([...node.childNodes], namespace);
        continue;
      }
      if (node.nodeType !== Node.COMMENT_NODE) continue;
      const value = node.textContent ?? "";
      const open = openMarker(node);
      if (open) {
        const id = open[1]!;
        if (!id.startsWith(namespace) || id.slice(namespace.length).includes(".") || seen.has(id))
          invalid();
        seen.add(id);
        stack.push({ id, namespace });
        if (open[2] === "e") namespace = `${id}.`;
      } else if (value.startsWith("/zr:1:")) {
        const frame = stack.pop();
        if (!frame || value !== `/zr:1:${frame.id}`) invalid();
        namespace = frame!.namespace;
      } else if (value.includes("zr:1:")) invalid();
    }
    if (stack.length) invalid();
  };
  visit(nodes, prefix);
}
