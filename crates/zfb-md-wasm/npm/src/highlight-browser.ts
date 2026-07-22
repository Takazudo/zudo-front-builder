import glueHref from "./wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.mjs";
import wasmHref from "./wasm-highlight/zfb_md_wasm_highlight_bg.wasm";
import { createWasmApi } from "./runtime.js";

// Mirrors src/browser.ts, but points at the highlight-only artifact under
// src/wasm-highlight/ (zfb#1849, epic zfb#1845) and re-exports only the
// highlight-surface API -- see src/highlight.ts for the direct/Node twin
// and why `compile`/`renderHtml` are absent here.
//
// These hrefs are emitted by esbuild's `file` loader. Resolving them from the
// final entry keeps the resource graph correct when the island entry is hashed
// or served under a non-root base path.
// TypeScript sees the generated module's normal wasm-bindgen default export,
// whereas esbuild's file loader replaces each value with a string URL. The
// cast documents that build-only boundary without changing the emitted import.
const GLUE_URL = new URL(glueHref as unknown as string, import.meta.url);
const WASM_URL = new URL(wasmHref as unknown as string, import.meta.url);

async function loadBrowserWasmBytes(): Promise<ArrayBuffer> {
  const response = await fetch(WASM_URL);
  if (!response.ok) {
    throw new Error(
      `zfb-md-wasm: failed to fetch wasm binary: ${response.status} ${response.statusText}`,
    );
  }
  return response.arrayBuffer();
}

const api = createWasmApi({
  glueUrl: GLUE_URL,
  loadWasmBytes: loadBrowserWasmBytes,
});

export const { init, highlightCode, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";

export type {
  HighlightRole,
  HighlightCodeOptions,
  HighlightCodeResult,
  HighlightDiagnostic,
  HighlightDiagnosticSource,
} from "./types.js";
