import { createDescription } from "./description.js";
import type { Child, ElementType } from "./description.js";

export { Fragment, isDescription, flattenChildren } from "./description.js";
export type { Key, Scalar, Child, Component, ElementType, Description } from "./description.js";
export type { ReadonlySignal, Signal } from "./reactive-types.js";

export type Cleanup = () => void;
export interface Scope {
  readonly abortSignal: AbortSignal;
  onActivate(fn: () => void | Cleanup): void;
  onCleanup(fn: Cleanup): void;
  effect(fn: () => void | Cleanup): void;
}
export interface Ref<T> {
  current: T | null;
}
export type Style = string | Readonly<Record<string, string | number | null | undefined>>;
export type Listener<E extends Event = Event> = (event: E) => void | Promise<void>;
export interface IslandIdentity {
  readonly component: string;
  readonly build: string;
}
export interface Diagnostic {
  readonly code: string;
  readonly phase: "setup" | "preflight" | "commit" | "activation" | "update" | "cleanup";
  readonly component: string;
  readonly componentStack: readonly string[];
  readonly path: string;
  readonly expected: string;
  readonly actual: string;
  readonly protocol: string;
  readonly build: string;
}
export type Reporter = (diagnostic: Diagnostic) => void;
export interface ShowProps {
  when: import("./reactive-types.js").ReadonlySignal<boolean>;
  children: () => Child;
  fallback?: (() => Child) | undefined;
}
export interface ForProps<T> {
  each: import("./reactive-types.js").ReadonlySignal<readonly T[]>;
  by: (item: T) => import("./description.js").Key;
  children: (
    item: import("./reactive-types.js").ReadonlySignal<T>,
    index: import("./reactive-types.js").ReadonlySignal<number>,
  ) => Child;
}

export function h(type: ElementType, props: Record<string, unknown> | null, ...children: Child[]) {
  const copy = { ...props };
  if (children.length > 0) copy.children = children.length === 1 ? children[0] : children;
  return createDescription(type, copy);
}
