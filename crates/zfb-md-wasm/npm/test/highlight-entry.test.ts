// Smoke test for the `./highlight` export subpath (zfb#1849, epic zfb#1845):
// the highlight-only wasm artifact, built with the crate's `pipeline`
// Cargo feature off (see crates/zfb-md-wasm/Cargo.toml). Proves the entry
// round-trips end to end against its OWN wasm-highlight artifact (not the
// default one -- see highlight-parity.test.ts for the byte-for-byte parity
// gate between the two), and that it exposes no `compile`/`renderHtml`.
import { describe, expect, it } from "vitest";

import * as highlightEntry from "../dist/highlight.js";
import {
  init,
  highlightCode,
  version,
  ZfbMdWasmTrapError,
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
} from "../dist/highlight.js";

describe("`./highlight` entry (highlight-only wasm artifact)", () => {
  it("exposes no compile/renderHtml -- only the highlight surface", () => {
    const exported = new Set(Object.keys(highlightEntry));
    expect(exported.has("compile")).toBe(false);
    expect(exported.has("renderHtml")).toBe(false);
    expect(exported.has("highlightCode")).toBe(true);
    expect(exported.has("init")).toBe(true);
    expect(exported.has("version")).toBe(true);
  });

  it("init() + highlightCode() + version() round-trip against the highlight-only artifact", async () => {
    await init();
    const semverPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
    expect(await version()).toMatch(semverPattern);

    const out = await highlightCode("const answer = 42;", { language: "javascript" });
    expect(out.html).toMatch(/^<pre class="hi-root"><code>/);
    expect(out.diagnostics).toEqual([]);
  });

  it("a wasm trap throws ZfbMdWasmTrapError and the next call transparently recovers", async () => {
    await init();
    await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);

    const recovered = await highlightCode("const recovered = true;", { language: "javascript" });
    expect(recovered.html).toContain("hi-kw");
    expect(recovered.diagnostics).toEqual([]);
    expect(__getTrapRecoveryStateForTests().terminal).toBe(false);
  });
});
