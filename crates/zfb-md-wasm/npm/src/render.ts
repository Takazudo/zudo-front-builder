import { createWasmApi } from "./runtime.js";

const GLUE_URL = new URL("./wasm-render/zfb_md_wasm_render_glue.zfb-resource.mjs", import.meta.url);
const WASM_URL = new URL("./wasm-render/zfb_md_wasm_render_bg.wasm", import.meta.url);

async function loadDirectWasmBytes(): Promise<ArrayBuffer> {
  if (typeof process !== "undefined" && process.versions?.node) {
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

const api = createWasmApi({ glueUrl: GLUE_URL, loadWasmBytes: loadDirectWasmBytes });

export const { init, renderHtml, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";
export type {
  RenderHtmlResult,
  Diagnostic,
  DiagnosticSource,
  ZfbMdWasmOptions,
  PipelineOptions,
  GfmOptions,
  CodeHighlightMode,
  CodeHighlightOptions,
  MarkdownFeaturesConfig,
  JsxRuntime,
  HighlightRole,
} from "./types.js";
