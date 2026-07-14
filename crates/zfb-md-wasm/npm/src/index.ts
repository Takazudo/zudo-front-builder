import { createWasmApi } from "./runtime.js";

const GLUE_URL = new URL("./wasm/zfb_md_wasm_glue.zfb-resource.mjs", import.meta.url);
const WASM_URL = new URL("./wasm/zfb_md_wasm_bg.wasm", import.meta.url);

function isNode(): boolean {
  return typeof process !== "undefined" && !!process.versions?.node;
}

// Node's fetch cannot load file: URLs, while direct browser/module consumers
// should keep fetching the files relative to this package entry. Browser-aware
// bundlers select browser.ts instead, which has static resource URL edges.
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
