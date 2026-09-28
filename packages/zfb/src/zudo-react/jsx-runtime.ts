import { createDescription } from "./description.js";
import type { Description, ElementType, Key } from "./description.js";

export { Fragment } from "./index.js";
export type { JSX } from "./jsx-types.js";

export function jsx(
  type: ElementType,
  props: Record<string, unknown> | null,
  key?: Key | undefined,
): Description {
  return createDescription(type, props, key);
}

export const jsxs: typeof jsx = jsx;
