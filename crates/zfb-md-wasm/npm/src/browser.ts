import glueHref from "./wasm/zfb_md_wasm_glue.zfb-resource.mjs";
import wasmHref from "./wasm/zfb_md_wasm_bg.wasm";
import { createWasmApi } from "./runtime.js";

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

export const {
  init,
  compile,
  renderHtml,
  highlightCode,
  version,
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
} = api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";

export type {
  CompileResult,
  RenderHtmlResult,
  Diagnostic,
  DiagnosticSource,
  ZfbMdWasmOptions,
  PipelineOptions,
  GfmOptions,
  MarkdownFeaturesConfig,
  JsxRuntime,
  HighlightRole,
  HighlightCodeOptions,
  HighlightCodeResult,
  HighlightDiagnostic,
  HighlightDiagnosticSource,
} from "./types.js";
