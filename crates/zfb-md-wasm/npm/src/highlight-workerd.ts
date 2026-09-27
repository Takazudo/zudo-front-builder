import wasmModule from "./wasm-highlight/zfb_md_wasm_highlight_bg.wasm";
import { createGlue } from "./wasm-highlight/zfb_md_wasm_highlight_glue.zfb-factory.mjs";
import { createStaticWasmApi } from "./runtime-static.js";

const api = createStaticWasmApi({ module: wasmModule as WebAssembly.Module, createGlue });

export const { init, highlightCode, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime-static.js";

export type {
  HighlightRole,
  HighlightCodeOptions,
  HighlightCodeResult,
  HighlightDiagnostic,
  HighlightDiagnosticSource,
} from "./types.js";
