import { createWasmApiFromStrategy } from "./runtime-core.js";
import type { WasmGlueModule } from "./runtime-core.js";

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime-core.js";

export interface StaticWasmResourceConfig {
  module: WebAssembly.Module | (() => WebAssembly.Module | Promise<WebAssembly.Module>);
  createGlue(): WasmGlueModule;
}

/** Create fresh glue instances around one statically supplied compiled module. */
export function createStaticWasmApi({ module, createGlue }: StaticWasmResourceConfig) {
  let modulePromise: Promise<WebAssembly.Module> | undefined;
  function getModule(onLoad: () => void): Promise<WebAssembly.Module> {
    if (!modulePromise) {
      onLoad();
      const attempt = Promise.resolve().then(() =>
        typeof module === "function" ? module() : module,
      );
      modulePromise = attempt;
      void attempt.catch(() => {
        if (modulePromise === attempt) modulePromise = undefined;
      });
    }
    return modulePromise;
  }

  return createWasmApiFromStrategy({
    getModule,
    createGlue,
  });
}
