import type {
  CompileResult,
  HighlightCodeOptions,
  HighlightCodeResult,
  ParseToAstOptions,
  ParseToAstResult,
  RenderHtmlResult,
  ZfbMdWasmOptions,
} from "./types.js";

export interface WasmGlueModule {
  // Keep this deliberately structural instead of importing the generated
  // glue's declaration file. The browser entry imports that file as a *URL
  // resource*, while the direct entry imports it dynamically. Both paths
  // nevertheless use the same wasm-bindgen surface and recovery
  // implementation below.
  //
  // Capability functions are structural and optional because each singleton
  // artifact's wasm-bindgen glue contains only its selected Rust export.
  // Public entries expose only the matching calls from `createWasmApi`.
  initSync(input?: { module: WebAssembly.Module }): unknown;
  compile?(source: string, optionsJson: string): string;
  renderHtml?(source: string, optionsJson: string): string;
  // Raw-mdast export (zfb#1857, epic zfb#1854).
  parseToAst?(source: string, optionsJson: string): string;
  highlightCode?(code: string, optionsJson: string): string;
  version(): string;
  __forceTrapForTests(): void;
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
        `unbounded recovery resource growth. Reload the JS realm before using zfb-md-wasm ` +
        `again, and please report the input that triggered the repeated traps.`,
    );
    this.name = "ZfbMdWasmTrapRecoveryLimitError";
    this.cause = cause;
  }
}

export interface WasmInstanceStrategy {
  getModule(onLoad: () => void): Promise<WebAssembly.Module>;
  createGlue(generation: number, attempt: number): WasmGlueModule | Promise<WasmGlueModule>;
}

/** Shared instance lifecycle and bounded trap recovery for URL and static entries. */
export function createWasmApiFromStrategy(strategy: WasmInstanceStrategy) {
  const MAX_TRAP_RECOVERIES = 16;

  let currentGeneration = 0;
  let trapRecoveriesStarted = 0;
  let freshInstanceStarts = 0;
  // For the static strategy, glueImportAttempts counts createGlue calls.
  let glueImportAttempts = 0;
  let compiledModuleLoads = 0;
  let terminalTrapRecoveryError: ZfbMdWasmTrapRecoveryLimitError | undefined;

  interface Instance {
    generation: number;
    glue: WasmGlueModule;
  }

  /**
   * Every fresh instance attempt creates new wasm-bindgen glue. The attempt
   * counter is independent of the trap generation: transient glue/initSync
   * failures must not consume the bounded trap-recovery budget.
   */
  async function freshInstance(generation: number): Promise<Instance> {
    freshInstanceStarts += 1;
    glueImportAttempts += 1;
    const [module, glue] = await Promise.all([
      strategy.getModule(() => {
        compiledModuleLoads += 1;
      }),
      // A synchronous factory throw is a retryable instance rejection.
      Promise.resolve().then(() => strategy.createGlue(generation, glueImportAttempts)),
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
    // await its replacement instead of creating another glue instance.
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

  // Public entry surfaces make missing calls unreachable. This guard keeps
  // structural glue mismatches artifact-neutral and actionable.
  function requireCapability<T>(fn: T | undefined, name: string): T {
    if (!fn) {
      throw new Error(
        `zfb-md-wasm: ${name}() is not available in this wasm artifact. ` +
          `Import an entry whose artifact provides that capability.`,
      );
    }
    return fn;
  }

  async function compile(source: string, options: ZfbMdWasmOptions = {}): Promise<CompileResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requireCapability(glue.compile, "compile").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as CompileResult;
  }

  async function renderHtml(
    source: string,
    options: ZfbMdWasmOptions = {},
  ): Promise<RenderHtmlResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requireCapability(glue.renderHtml, "renderHtml").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as RenderHtmlResult;
  }

  /**
   * Parse markdown/MDX into a raw mdast tree (zfb#1857, epic zfb#1854).
   * The `parseToAst + JSON.parse` round trip here IS the product cost the
   * epic's benchmark measures against remark-parse. See `types.ts`'s
   * `ParseToAstResult`/`MdastNode` docs for the result shape and the
   * UTF-16 position contract.
   */
  async function parseToAst(
    source: string,
    options: ParseToAstOptions = {},
  ): Promise<ParseToAstResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requireCapability(glue.parseToAst, "parseToAst").call(glue, source, optionsJson),
    );
    return JSON.parse(json) as ParseToAstResult;
  }

  async function highlightCode(
    code: string,
    options: HighlightCodeOptions,
  ): Promise<HighlightCodeResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) =>
      requireCapability(glue.highlightCode, "highlightCode").call(glue, code, optionsJson),
    );
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
