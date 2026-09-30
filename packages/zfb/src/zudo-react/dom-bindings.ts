import { subscribe, readSnapshot, type Subscription } from "./reactive.js";
import type { ReadonlySignal } from "./reactive-types.js";
import type { RuntimeScope } from "./scope.js";
import type { RootOptions } from "./root.js";
import { diagnostic, report, rootPath } from "./root.js";
import { booleanAttrs } from "./vocabulary.js";

export function isReactive(value: unknown): value is ReadonlySignal<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "$$zudoReactive" in value &&
    value.$$zudoReactive === "zudo-react.reactive.v1"
  );
}
export function read(value: unknown): unknown {
  return isReactive(value) ? readSnapshot(value) : value;
}
export function styleText(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new TypeError("ZR_STYLE");
  let result = "";
  for (const [name, entry] of Object.entries(value)) {
    if (!/^(--[a-zA-Z0-9_-]+|[a-z][a-z0-9-]*)$/.test(name) || /[A-Z]/.test(name))
      throw new TypeError(`ZR_STYLE: ${name}`);
    if (entry == null) continue;
    if (typeof entry !== "string" && !(typeof entry === "number" && Number.isFinite(entry)))
      throw new TypeError(`ZR_STYLE: ${name}`);
    result += `${name}:${entry};`;
  }
  return result;
}
export function setAttribute(element: Element, name: string, value: unknown): void {
  if (value == null || (booleanAttrs.has(name) && value === false)) {
    element.removeAttribute(name);
    return;
  }
  const text = name === "style" ? styleText(value) : booleanAttrs.has(name) ? "" : String(value);
  if (element.getAttribute(name) !== text) element.setAttribute(name, text);
}
export function bind<T>(
  source: ReadonlySignal<T>,
  scope: RuntimeScope,
  options: RootOptions,
  container: Element,
  path: string,
  apply: (value: T) => void,
  initial: T,
): Subscription {
  let previous = initial;
  const subscription = subscribe(
    () => {
      const next = source.value;
      if (Object.is(previous, next)) return;
      previous = next;
      apply(next);
    },
    {
      active: () => scope.active,
      report: (code, actual) =>
        report(
          options,
          diagnostic(
            options,
            code,
            "update",
            `${rootPath(container)}${path}`,
            "binding update",
            actual,
          ),
        ),
    },
  );
  subscription.run();
  return subscription;
}
export function listener(name: string): { name: string; capture: boolean } | null {
  const match = /^on:([A-Za-z][A-Za-z0-9_-]*)(:capture)?$/.exec(name);
  return match ? { name: match[1]!, capture: Boolean(match[2]) } : null;
}
