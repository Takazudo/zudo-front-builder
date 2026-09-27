import { createWasmApiFromStrategy } from "./runtime-core.js";
import type { WasmGlueModule } from "./runtime-core.js";

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime-core.js";

export interface WasmResourceConfig {
  glueUrl: URL;
  loadWasmBytes(): Promise<ArrayBuffer>;
  /** @internal Deterministic test seam; production uses WebAssembly.compile. */
  compileWasm?(bytes: ArrayBuffer): Promise<WebAssembly.Module>;
  /** @internal Deterministic test seam; production dynamically imports the glue URL. */
  importGlue?(specifier: string): Promise<WasmGlueModule>;
}

/** URL loading strategy used by the existing browser and direct entries. */
export function createWasmApi({
  glueUrl,
  loadWasmBytes,
  compileWasm = (bytes) => WebAssembly.compile(bytes),
  importGlue = (specifier) => import(/* @vite-ignore */ specifier) as Promise<WasmGlueModule>,
}: WasmResourceConfig) {
  let compiledModulePromise: Promise<WebAssembly.Module> | undefined;
  function getModule(onLoad: () => void): Promise<WebAssembly.Module> {
    if (!compiledModulePromise) {
      onLoad();
      const attempt = Promise.resolve().then(loadWasmBytes).then(compileWasm);
      compiledModulePromise = attempt;
      void attempt.catch(() => {
        // Only the rejected attempt may clear its own cache slot.
        if (compiledModulePromise === attempt) compiledModulePromise = undefined;
      });
    }
    return compiledModulePromise;
  }

  return createWasmApiFromStrategy({
    getModule,
    createGlue(generation, attempt) {
      const glueSpecifier = new URL(glueUrl.href);
      glueSpecifier.searchParams.set("zfbMdWasmGen", String(generation));
      glueSpecifier.searchParams.set("zfbMdWasmAttempt", String(attempt));
      return importGlue(glueSpecifier.href);
    },
  });
}
