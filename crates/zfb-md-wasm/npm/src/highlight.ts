import { createWasmApi } from "./runtime.js";

// Mirrors src/index.ts, but points at the highlight-only artifact under
// src/wasm-highlight/ (zfb#1849, epic zfb#1845) and re-exports only the
// highlight-surface API -- no `compile`/`renderHtml`, which the underlying
// wasm module doesn't even export (built with the `pipeline` Cargo feature
// off, see crates/zfb-md-wasm/Cargo.toml).
const GLUE_URL = new URL(
  "./wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.mjs",
  import.meta.url,
);
const WASM_URL = new URL("./wasm-highlight/zfb_md_wasm_highlight_bg.wasm", import.meta.url);

function isNode(): boolean {
  return typeof process !== "undefined" && !!process.versions?.node;
}

// Node's fetch cannot load file: URLs, while direct browser/module consumers
// should keep fetching the files relative to this package entry. Browser-aware
// bundlers select highlight-browser.ts instead, which has static resource URL
// edges.
async function loadDirectWasmBytes(): Promise<ArrayBuffer> {
  if (isNode()) {
    const [{ readFile }, { fileURLToPath }] = await Promise.all([
      import("node:fs/promises"),
      import("node:url"),
    ]);
    const bytes = await readFile(fileURLToPath(WASM_URL));
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  }

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
  loadWasmBytes: loadDirectWasmBytes,
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
