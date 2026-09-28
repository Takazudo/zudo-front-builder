import type { ReadonlySignal } from "./reactive-types.js";

export type Key = string | number;
export type Scalar = string | number | boolean | null | undefined;
export type Child = Scalar | Description | ReadonlySignal<Scalar> | readonly Child[];
export type Component<P = Record<string, unknown>> = (props: P) => Child;
export const Fragment: unique symbol = Symbol.for(
  "@takazudo/zfb/zudo-react/fragment-v1",
) as typeof Fragment;
export type ElementType =
  | string
  | Component<any>
  | typeof import("../island.js").Island
  | typeof Fragment;

export interface Description {
  readonly $$zudo: "zudo-react.description.v1";
  readonly type: ElementType;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: Key | null;
}

function valueKind(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (typeof value === "object" && value instanceof Promise) return "promise";
  return typeof value;
}

function validKey(value: unknown): value is Key {
  return typeof value === "string" || (typeof value === "number" && Number.isFinite(value));
}

export function createDescription(
  type: ElementType,
  props: Record<string, unknown> | null,
  key?: Key | undefined,
): Description {
  const copy = { ...props };
  const resolved = key === undefined ? copy.key : key;
  if (resolved !== undefined && resolved !== null && !validKey(resolved)) {
    throw new TypeError(`Invalid key: ${valueKind(resolved)}`);
  }
  delete copy.key;
  return {
    $$zudo: "zudo-react.description.v1",
    type,
    props: copy,
    key: resolved == null ? null : resolved,
  };
}

export function isDescription(value: unknown): value is Description {
  return (
    typeof value === "object" &&
    value !== null &&
    "$$zudo" in value &&
    value.$$zudo === "zudo-react.description.v1" &&
    "type" in value &&
    "props" in value &&
    "key" in value
  );
}

function isReactive(value: unknown): value is ReadonlySignal<Scalar> {
  return (
    typeof value === "object" &&
    value !== null &&
    "$$zudoReactive" in value &&
    value.$$zudoReactive === "zudo-react.reactive.v1"
  );
}

export function flattenChildren(
  children: Child,
): readonly Exclude<Child, readonly Child[] | boolean | null | undefined>[] {
  const result: Exclude<Child, readonly Child[] | boolean | null | undefined>[] = [];
  function visit(value: unknown, path: string): void {
    if (value === null || value === undefined || typeof value === "boolean") return;
    if (Array.isArray(value)) {
      value.forEach((child, index) => visit(child, `${path}[${index}]`));
      return;
    }
    if (
      typeof value === "string" ||
      (typeof value === "number" && Number.isFinite(value)) ||
      isDescription(value) ||
      isReactive(value)
    ) {
      result.push(value as (typeof result)[number]);
      return;
    }
    throw new TypeError(`Invalid child ${valueKind(value)} at ${path} in parent <root>`);
  }
  visit(children, "children");
  return result;
}
