/// <reference lib="dom" />
export type Fallback = "none" | "animate" | "swap";
export type Direction = "forward" | "back";
export type NavigationTypeString = "push" | "replace" | "traverse";
export type Options = {
  history?: "auto" | "push" | "replace";
  info?: any;
  state?: any;
  formData?: FormData;
  sourceElement?: Element; // more than HTMLElement, e.g. SVGAElement
};

// Public options for syncHistoryEntry() (#1377).
export type SyncHistoryEntryOptions = {
  // Replace the current history entry instead of pushing a new one.
  replace?: boolean;
  // Consumer state merged into the entry; the router's bookkeeping keys
  // (index/scrollX/scrollY) always take precedence over colliding keys.
  state?: any;
};
