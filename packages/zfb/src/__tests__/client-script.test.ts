// Tests for the `clientScript()` SSR helper (#978).
//
// The helper reads `globalThis.__zfb.base` (set by the bundler when client
// scripts are present) and prepends it to the stable `/assets/client/<name>.js`
// URL. These tests cover:
//
// 1. `__zfb.base` not set at all (missing `__zfb` slot).
// 2. `__zfb.base` set to `""` (root-mounted / no-base build).
// 3. `__zfb.base` set to a sub-path prefix (e.g. `"/foo"`).
// 4. `__zfb` present but `base` field absent (other slots set, `base` never
//    emitted — zero-script fallback or an older bundle).

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clientScript } from "../client-script.js";

// We need to manipulate globalThis.__zfb between tests.
// Vitest runs in Node (globalThis is the global object).

type ZfbGlobal = { base?: string; [key: string]: unknown };

function setBase(value: string | undefined): void {
  const g = globalThis as Record<string, unknown>;
  if (value === undefined) {
    // Ensure __zfb exists but without the `base` field.
    g.__zfb = {};
  } else {
    (g.__zfb as ZfbGlobal) = { base: value };
  }
}

function clearZfb(): void {
  const g = globalThis as Record<string, unknown>;
  delete g.__zfb;
}

describe("clientScript()", () => {
  // Clean up after each test so globals don't leak between cases.
  beforeEach(() => {
    clearZfb();
  });
  afterEach(() => {
    clearZfb();
  });

  it("returns the bare stable URL when __zfb is not set at all", () => {
    // Zero-script projects never emit globalThis.__zfb.base; the helper
    // must still return a valid URL (the unprefixed stable URL).
    expect(clientScript("search-widget")).toBe("/assets/client/search-widget.js");
  });

  it("returns the bare stable URL when __zfb.base is empty string (root mount)", () => {
    // Root-mounted build: bundler emits `globalThis.__zfb.base = ""`.
    setBase("");
    expect(clientScript("search-widget")).toBe("/assets/client/search-widget.js");
  });

  it("prepends the sub-path prefix when __zfb.base is set", () => {
    // Sub-path deploy: bundler emits `globalThis.__zfb.base = "/foo"`.
    setBase("/foo");
    expect(clientScript("search-widget")).toBe("/foo/assets/client/search-widget.js");
  });

  it("handles nested sub-path prefixes", () => {
    setBase("/pj/zudo-doc");
    expect(clientScript("analytics")).toBe("/pj/zudo-doc/assets/client/analytics.js");
  });

  it("returns the bare stable URL when __zfb exists but base field is absent", () => {
    // The `base` field is only emitted when client scripts are present.
    // If __zfb was set by another setter (e.g. `site`) but not `base`,
    // clientScript() should fall back to the unprefixed URL.
    setBase(undefined); // sets __zfb = {}
    expect(clientScript("search-widget")).toBe("/assets/client/search-widget.js");
  });

  it("uses the entry name verbatim with the /assets/client/ prefix", () => {
    // The name is the file-stem minus `.client`, e.g. `search-widget` for
    // `search-widget.client.ts`. The helper must not add extra slashes or
    // modify the name.
    setBase("");
    expect(clientScript("my-lib")).toBe("/assets/client/my-lib.js");
    expect(clientScript("analytics")).toBe("/assets/client/analytics.js");
  });

  it("does not collide with the islands or CSS stable URLs", () => {
    setBase("");
    const url = clientScript("styles"); // even if the name is "styles"
    expect(url).not.toBe("/assets/styles.css");
    expect(url).not.toBe("/assets/islands.js");
    expect(url).toBe("/assets/client/styles.js");
  });
});
