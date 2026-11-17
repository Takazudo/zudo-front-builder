/** Internal legacy paths() shape; intentionally permissive and mutable. */
export interface PathsEntry {
  readonly params: Record<string, unknown>;
  readonly props?: Record<string, unknown>;
}

/**
 * Keep the legacy validation unchanged. This module boundary also lets tests
 * observe actual validation calls without a shipped profiler or runtime flags.
 */
export function isPathsEntry(x: unknown): x is PathsEntry {
  if (typeof x !== "object" || x === null) return false;
  const params = (x as { params?: unknown }).params;
  return typeof params === "object" && params !== null;
}
