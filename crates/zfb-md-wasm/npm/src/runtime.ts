import type {
  CompileResult,
  HighlightCodeOptions,
  HighlightCodeResult,
  ParseToAstResult,
  RenderHtmlResult,
  ZfbMdWasmOptions,
} from "./types.js";

interface WasmGlueModule {
  // Keep this deliberately structural instead of importing the generated
  // glue's declaration file. The browser entry imports that file as a *URL
  // resource*, while the direct entry imports it dynamically. Both paths
  // nevertheless use the same wasm-bindgen surface and recovery
  // implementation below.
  //
  // `compile`/`renderHtml` are optional: the highlight-only wasm artifact
  // (`./highlight` entry, zfb#1849, epic zfb#1845) is compiled with the
  // `pipeline` Cargo feature off (see crates/zfb-md-wasm/Cargo.toml), so its
  // wasm-bindgen glue never generates these exports at all. `createWasmApi`
  // below is shared by both entries; only `src/index.ts`/`src/browser.ts`
  // re-export the functions that call them.
  initSync(input?: { module: WebAssembly.Module }): unknown;
  compile?(source: string, optionsJson: string): string;
  renderHtml?(source: string, optionsJson: string): string;
  // PROTOTYPE (zfb#1855, epic zfb#1854): raw-mdast export spike; pipeline-
  // gated like compile/renderHtml, pruned on a Wave-2 no-go.
  parseToAst?(source: string, optionsJson: string): string;
  highlightCode(code: string, optionsJson: string): string;
  version(): string;
  __forceTrapForTests(): void;
}

export interface WasmResourceConfig {
  glueUrl: URL;
  loadWasmBytes(): Promise<ArrayBuffer>;
  /** @internal Deterministic test seam; production uses WebAssembly.compile. */
  compileWasm?(bytes: ArrayBuffer): Promise<WebAssembly.Module>;
  /** @internal Deterministic test seam; production dynamically imports the glue URL. */
  importGlue?(specifier: string): Promise<WasmGlueModule>;
}

export class ZfbMdWasmTrapError extends Error {
  constructor(cause: unknown) {
    super(
      "zfb-md-wasm: the wasm instance trapped (a Rust panic or internal fault) and has been " +
        "automatically re-instantiated. This is always a bug in zfb-md-wasm -- please report it " +
        "with the input that triggered it.",
    );
    this.name = "ZfbMdWasmTrapError";
    this.cause = cause;
  }
}

export class ZfbMdWasmTrapRecoveryLimitError extends Error {
  constructor(maxRecoveries: number, cause: unknown) {
    super(
      `zfb-md-wasm: wasm trap recovery limit reached after ${maxRecoveries} ` +
        `successful re-instantiations. Further automatic recovery is disabled to avoid ` +
        `unbounded ES module record growth. Reload the JS realm before using zfb-md-wasm ` +
        `again, and please report the input that triggered the repeated traps.`,
    );
    this.name = "ZfbMdWasmTrapRecoveryLimitError";
    this.cause = cause;
  }
}

export function createWasmApi({
  glueUrl,
  loadWasmBytes,
  compileWasm = (bytes) => WebAssembly.compile(bytes),
  importGlue = (specifier) => import(/* @vite-ignore */ specifier) as Promise<WasmGlueModule>,
}: WasmResourceConfig) {
  let compiledModulePromise: Promise<WebAssembly.Module> | undefined;
  let compiledModuleLoads = 0;

  function getCompiledModule(): Promise<WebAssembly.Module> {
    if (!compiledModulePromise) {
      compiledModuleLoads += 1;
      const attempt = Promise.resolve().then(loadWasmBytes).then(compileWasm);
      compiledModulePromise = attempt;
      void attempt.catch(() => {
        // A rejected attempt must be retryable, but it must not erase a newer
        // attempt if its rejection cleanup runs late.
        if (compiledModulePromise === attempt) {
          compiledModulePromise = undefined;
        }
      });
    }
    return compiledModulePromise;
  }

  const MAX_TRAP_RECOVERIES = 16;

  let currentGeneration = 0;
  let trapRecoveriesStarted = 0;
  let freshInstanceStarts = 0;
  let glueImportAttempts = 0;
  let terminalTrapRecoveryError: ZfbMdWasmTrapRecoveryLimitError | undefined;

  interface Instance {
    generation: number;
    glue: WasmGlueModule;
  }

  /**
   * Every fresh instance attempt creates a new wasm-bindgen glue module
   * record. The import-attempt nonce is deliberately independent of the trap
   * generation: transient import/initSync failures need a new module record
   * without consuming the bounded trap-recovery budget.
   */
  async function freshInstance(generation: number): Promise<Instance> {
    freshInstanceStarts += 1;
    glueImportAttempts += 1;
    const glueSpecifier = new URL(glueUrl.href);
    glueSpecifier.searchParams.set("zfbMdWasmGen", String(generation));
    glueSpecifier.searchParams.set("zfbMdWasmAttempt", String(glueImportAttempts));
    const [module, glue] = await Promise.all([
      getCompiledModule(),
      // Deferring the injected function also converts a synchronous test-seam
      // throw into the rejection handled by the retryable instance cache.
      Promise.resolve().then(() => importGlue(glueSpecifier.href)),
    ]);
    glue.initSync({ module });
    return { generation, glue };
  }

  let instancePromise: Promise<Instance> | undefined;

  function startInstance(generation: number): Promise<Instance> {
    const attempt = freshInstance(generation);
    instancePromise = attempt;
    void attempt.catch(() => {
      // Trap recovery can replace the installed instance promise. Never let a
      // stale initialization rejection clear that newer attempt.
      if (instancePromise === attempt) {
        instancePromise = undefined;
      }
    });
    return attempt;
  }

  function getInstance(): Promise<Instance> {
    if (terminalTrapRecoveryError) {
      return Promise.reject(terminalTrapRecoveryError);
    }
    if (!instancePromise) {
      return startInstance(currentGeneration);
    }
    return instancePromise;
  }

  function isTrap(err: unknown): boolean {
    return typeof WebAssembly !== "undefined" && err instanceof WebAssembly.RuntimeError;
  }

  async function recoverAfterTrap(observedGeneration: number, cause: unknown): Promise<void> {
    if (terminalTrapRecoveryError) {
      throw terminalTrapRecoveryError;
    }

    // CAS-style single-flight: reporters for an already-replaced generation
    // await its replacement instead of creating another glue module record.
    if (observedGeneration !== currentGeneration) {
      await getInstance();
      return;
    }

    if (trapRecoveriesStarted >= MAX_TRAP_RECOVERIES) {
      terminalTrapRecoveryError = new ZfbMdWasmTrapRecoveryLimitError(MAX_TRAP_RECOVERIES, cause);
      instancePromise = undefined;
      throw terminalTrapRecoveryError;
    }

    trapRecoveriesStarted += 1;
    currentGeneration += 1;
    await startInstance(currentGeneration);
  }

  async function callWasm<T>(fn: (instance: Instance) => T): Promise<T> {
    const instance = await getInstance();
    try {
      return fn(instance);
    } catch (err) {
      if (!isTrap(err)) {
        throw err;
      }
      await recoverAfterTrap(instance.generation, err);
      throw new ZfbMdWasmTrapError(err);
    }
  }

  async function init(): Promise<void> {
    await getInstance();
  }

  // Both throw helpers below fire only if `compile`/`renderHtml` were
  // somehow invoked against the highlight-only glue -- unreachable through
  // the public API surface, since `src/highlight.ts`/`src/highlight-browser.ts`
  // never re-export these two functions. Kept as a clear runtime error
  // rather than a silent `undefined()` crash.
  function requirePipelineExport<T>(fn: T | undefined, name: string): T {
    if (!fn) {
      throw new Error(
        `zfb-md-wasm: ${name}() is not available in the highlight-only wasm artifact ` +
          `(built with the \`pipeline\` Cargo feature off) -- use the default \`.\` entry instead.`,
      );
    }
    return fn;
  }

  async function compile(source: string, options: ZfbMdWasmOptions = {}): Promise<CompileResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requirePipelineExport(glue.compile, "compile").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as CompileResult;
  }

  async function renderHtml(
    source: string,
    options: ZfbMdWasmOptions = {},
  ): Promise<RenderHtmlResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requirePipelineExport(glue.renderHtml, "renderHtml").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as RenderHtmlResult;
  }

  /**
   * PROTOTYPE (zfb#1855, epic zfb#1854 — go/no-go spike; NOT a documented
   * or supported API surface): parse markdown/MDX into a raw mdast tree.
   * The `parseToAst + JSON.parse` round trip here IS the product cost the
   * epic's benchmark measures. Pruned on a Wave-2 no-go.
   */
  async function parseToAst(
    source: string,
    options: ZfbMdWasmOptions = {},
  ): Promise<ParseToAstResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requirePipelineExport(glue.parseToAst, "parseToAst").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as ParseToAstResult;
  }

  async function highlightCode(
    code: string,
    options: HighlightCodeOptions,
  ): Promise<HighlightCodeResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) => glue.highlightCode(code, optionsJson));
    return JSON.parse(json) as HighlightCodeResult;
  }

  async function version(): Promise<string> {
    return callWasm(({ glue }) => glue.version());
  }

  /** @internal Test-only hook that forces the current instance to trap. */
  async function __forceTrapForTests(): Promise<void> {
    await callWasm(({ glue }) => glue.__forceTrapForTests());
  }

  /** @internal Test-only observability for the bounded recovery contract. */
  function __getTrapRecoveryStateForTests(): {
    compiledModuleLoads: number;
    currentGeneration: number;
    freshInstanceStarts: number;
    glueImportAttempts: number;
    maxTrapRecoveries: number;
    trapRecoveriesStarted: number;
    terminal: boolean;
  } {
    return {
      compiledModuleLoads,
      currentGeneration,
      freshInstanceStarts,
      glueImportAttempts,
      maxTrapRecoveries: MAX_TRAP_RECOVERIES,
      trapRecoveriesStarted,
      terminal: !!terminalTrapRecoveryError,
    };
  }

  return {
    init,
    compile,
    renderHtml,
    parseToAst,
    highlightCode,
    version,
    __forceTrapForTests,
    __getTrapRecoveryStateForTests,
  };
}
