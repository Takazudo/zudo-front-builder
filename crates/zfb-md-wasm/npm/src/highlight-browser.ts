import glueHref from "./wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.mjs?url";
import wasmHref from "./wasm-highlight/zfb_md_wasm_highlight_bg.wasm?url";
import { createWasmApi } from "./runtime.js";

// Mirrors src/browser.ts, but points at the highlight-only artifact under
// src/wasm-highlight/ (zfb#1849, epic zfb#1845) and re-exports only the
// highlight-surface API -- see src/highlight.ts for the direct/Node twin
// and why `compile`/`renderHtml` are absent here.
//
// `?url` is the explicit cross-bundler asset contract: both Vite and zfb's
// pinned esbuild file loaders replace these imports with URL strings. Resolving
// them from the final entry keeps relative esbuild output correct while also
// accepting Vite's base-prefixed URLs.
const GLUE_URL = new URL(glueHref, import.meta.url);
const WASM_URL = new URL(wasmHref, import.meta.url);

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
