import type {
  CompileResult,
  HighlightCodeOptions,
  HighlightCodeResult,
  RenderHtmlResult,
  ZfbMdWasmOptions,
} from "./types.js";

// Keep this deliberately structural instead of importing the generated glue's
// declaration file. The browser entry imports that file as a *URL resource*,
// while the direct entry imports it dynamically. Both paths nevertheless use
// the same wasm-bindgen surface and recovery implementation below.
interface WasmRawExports {
  compile(retptr: number, a: number, b: number, c: number, d: number): void;
}

interface WasmGlueModule {
  initSync(input?: { module: WebAssembly.Module }): WasmRawExports;
  compile(source: string, optionsJson: string): string;
  renderHtml(source: string, optionsJson: string): string;
  highlightCode(code: string, optionsJson: string): string;
  version(): string;
}

export interface WasmResourceConfig {
  glueUrl: URL;
  loadWasmBytes(): Promise<ArrayBuffer>;
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

export function createWasmApi({ glueUrl, loadWasmBytes }: WasmResourceConfig) {
  let compiledModulePromise: Promise<WebAssembly.Module> | undefined;
  let compiledModuleLoads = 0;

  function getCompiledModule(): Promise<WebAssembly.Module> {
    if (!compiledModulePromise) {
      compiledModuleLoads += 1;
      compiledModulePromise = loadWasmBytes().then((bytes) => WebAssembly.compile(bytes));
    }
    return compiledModulePromise;
  }

  const MAX_TRAP_RECOVERIES = 16;

  let currentGeneration = 0;
  let trapRecoveriesStarted = 0;
  let freshInstanceStarts = 0;
  let terminalTrapRecoveryError: ZfbMdWasmTrapRecoveryLimitError | undefined;

  interface Instance {
    generation: number;
    glue: WasmGlueModule;
    raw: WasmRawExports;
  }

  /**
   * A fresh query creates a new wasm-bindgen glue module record after a real
   * trap. The compiled WebAssembly.Module remains cached, so recovery only
   * re-instantiates it. `generation` is wrapper-private and bounded below.
   */
  async function freshInstance(generation: number): Promise<Instance> {
    freshInstanceStarts += 1;
    const [module, glue] = await Promise.all([
      getCompiledModule(),
      import(
        /* @vite-ignore */ `${glueUrl.href}?zfbMdWasmGen=${generation}`
      ) as Promise<WasmGlueModule>,
    ]);
    const raw = glue.initSync({ module });
    return { generation, glue, raw };
  }

  let instancePromise: Promise<Instance> | undefined;

  function getInstance(): Promise<Instance> {
    if (terminalTrapRecoveryError) {
      return Promise.reject(terminalTrapRecoveryError);
    }
    instancePromise ??= freshInstance(currentGeneration);
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
    instancePromise = freshInstance(currentGeneration);
    await instancePromise;
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

  async function compile(source: string, options: ZfbMdWasmOptions = {}): Promise<CompileResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) => glue.compile(source, optionsJson));
    return JSON.parse(json) as CompileResult;
  }

  async function renderHtml(
    source: string,
    options: ZfbMdWasmOptions = {},
  ): Promise<RenderHtmlResult> {
    const optionsJson = JSON.stringify(options);
    const json = await callWasm(({ glue }) => glue.renderHtml(source, optionsJson));
    return JSON.parse(json) as RenderHtmlResult;
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
    await callWasm(({ raw }) => {
      raw.compile(0xfffffff0, 0, 0, 0, 0);
    });
  }

  /** @internal Test-only observability for the bounded recovery contract. */
  function __getTrapRecoveryStateForTests(): {
    compiledModuleLoads: number;
    currentGeneration: number;
    freshInstanceStarts: number;
    maxTrapRecoveries: number;
    trapRecoveriesStarted: number;
    terminal: boolean;
  } {
    return {
      compiledModuleLoads,
      currentGeneration,
      freshInstanceStarts,
      maxTrapRecoveries: MAX_TRAP_RECOVERIES,
      trapRecoveriesStarted,
      terminal: !!terminalTrapRecoveryError,
    };
  }

  return {
    init,
    compile,
    renderHtml,
    highlightCode,
    version,
    __forceTrapForTests,
    __getTrapRecoveryStateForTests,
  };
}
