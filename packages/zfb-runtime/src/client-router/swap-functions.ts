/// <reference lib="dom" />
// Stub — full implementation lands in W3B (client-router/swap-functions).
// This file exists solely so events.ts can import `swap` and typecheck in W3A.
// W3B replaces this file entirely with the ported swap-functions logic.

export function swap(_newDocument: Document): void {
  throw new Error("swap-functions not yet implemented — pending W3B");
}
