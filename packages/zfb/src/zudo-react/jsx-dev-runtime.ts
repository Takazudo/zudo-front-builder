import { createDescription, recordSite } from "./description.js";
import type { CompilerSource, Description, ElementType, Key } from "./description.js";

export { Fragment } from "./index.js";
export type { JSX } from "./jsx-types.js";

export function jsxDEV(
  type: ElementType,
  props: Record<string, unknown> | null,
  key: Key | undefined,
  _isStaticChildren: boolean,
  source: CompilerSource | undefined,
  _self: unknown,
): Description {
  return recordSite(createDescription(type, props, key), source, jsxDEV);
}
