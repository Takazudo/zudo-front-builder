import glueHref from "./wasm-render/zfb_md_wasm_render_glue.zfb-resource.mjs?url";
import wasmHref from "./wasm-render/zfb_md_wasm_render_bg.wasm?url";
import { createWasmApi } from "./runtime.js";

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

const api = createWasmApi({ glueUrl: GLUE_URL, loadWasmBytes: loadBrowserWasmBytes });

export const { init, renderHtml, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";
export type {
  RenderHtmlResult,
  Diagnostic,
  DiagnosticSource,
  ZfbMdWasmOptions,
  ParseDialect,
  PipelineOptions,
  GfmOptions,
  CodeHighlightMode,
  CodeHighlightOptions,
  MarkdownFeaturesConfig,
  JsxRuntime,
  HighlightRole,
} from "./types.js";
