// Type declarations for the "zfb" SDK module.
//
// At build time the zfb-render runtime resolves the bare specifier "zfb"
// to the JS module at ./zfb-sdk.js. Pages can import helpers like:
//   import { paginate } from "zfb";

export interface PaginateInput<T> {
  items: T[];
  pageSize: number;
}

export interface PaginatedPath<T> {
  params: { page: string };
  props: {
    current: number;
    total: number;
    pageSize: number;
    items: T[];
  };
}

/**
 * Split `items` into page slices of size `pageSize` and return a
 * paths()-shaped array. The returned array always contains at least one
 * entry — for an empty `items` array, a single page is returned with
 * `items: []`.
 */
export function paginate<T>(input: PaginateInput<T>): PaginatedPath<T>[];

/** SDK version string. Subject to change while zfb is pre-1.0. */
export const __zfbVersion: string;
