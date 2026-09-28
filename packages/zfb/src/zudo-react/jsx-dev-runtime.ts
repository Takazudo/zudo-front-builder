import { createDescription } from "./description.js";
import type { Description, ElementType, Key } from "./description.js";

export { Fragment } from "./index.js";
export type { JSX } from "./jsx-types.js";

export function jsxDEV(
  type: ElementType,
  props: Record<string, unknown> | null,
  key: Key | undefined,
  _isStaticChildren: boolean,
  _source: { fileName: string; lineNumber: number; columnNumber: number } | undefined,
  _self: unknown,
): Description {
  return createDescription(type, props, key);
}
