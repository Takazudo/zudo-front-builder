import { createDescription, type Child, type Description, type Key } from "./description.js";
import { computed, signal } from "./reactive.js";
import type { ReadonlySignal, Signal } from "./reactive-types.js";
import type { ShowProps, ForProps } from "./index.js";

export function Show(props: ShowProps): Description {
  return createDescription(Show, props as unknown as Record<string, unknown>);
}

export function For<T>(props: ForProps<T>): Description {
  return createDescription(For, props as unknown as Record<string, unknown>);
}

export function showProps(description: Description): ShowProps {
  return description.props as unknown as ShowProps;
}

export function forProps<T>(description: Description): ForProps<T> {
  return description.props as unknown as ForProps<T>;
}

export function keyed<T>(items: readonly T[], by: (item: T) => Key, path: string): Key[] {
  const seen = new Set<Key>();
  return items.map((item) => {
    const key = by(item);
    if (typeof key !== "string" && !(typeof key === "number" && Number.isFinite(key)))
      throw new TypeError(`ZR_KEY_TYPE: For at ${path}, key ${String(key)}`);
    if (seen.has(key)) throw new TypeError(`ZR_DUPLICATE_KEY: For at ${path}, key ${String(key)}`);
    seen.add(key);
    return key;
  });
}

export function keyPayload(key: Key): string {
  const bytes = new TextEncoder().encode(String(key));
  return `${typeof key === "number" ? "n" : "s"}${Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")}`;
}

export function view<T>(initial: T): { readonly value: ReadonlySignal<T>; write(next: T): void } {
  const source: Signal<T> = signal(initial);
  return { value: computed(() => source.value), write: (next) => (source.value = next) };
}

export type Factory = () => Child;
