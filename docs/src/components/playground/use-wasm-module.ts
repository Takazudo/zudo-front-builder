import { useEffect, useMemo, useRef, useState } from "preact/hooks";

/** One retryable promise cache. Create one loader for each WASM entry point. */
export function createModuleLoader<T>(load: () => Promise<T>): () => Promise<T> {
  let modulePromise: Promise<T> | null = null;

  return function loadModule(): Promise<T> {
    if (modulePromise === null) {
      const pending = load().catch((error: unknown) => {
        if (modulePromise === pending) {
          modulePromise = null;
        }
        throw error;
      });
      modulePromise = pending;
    }

    return modulePromise;
  };
}

export type WasmModuleState<TResult> =
  | { status: "idle"; result: null; error: null }
  | { status: "loading"; result: null; error: null }
  | { status: "ready"; result: TResult; error: null }
  | { status: "error"; result: null; error: unknown };

export interface WasmModuleRunner<TArgs extends unknown[], TResult> {
  run: (...args: TArgs) => Promise<TResult>;
  retry: () => Promise<TResult>;
  dispose: () => void;
}

export interface CreateWasmModuleRunnerOptions<TModule, TArgs extends unknown[], TResult> {
  loadModule: () => Promise<TModule>;
  execute: (module: TModule, ...args: TArgs) => Promise<TResult> | TResult;
  onStateChange: (state: WasmModuleState<TResult>) => void;
}

/**
 * Runs use last-started-run-wins semantics: every call proceeds, but stale
 * completions cannot replace the state belonging to a newer call.
 */
export function createWasmModuleRunner<TModule, TArgs extends unknown[], TResult>({
  loadModule,
  execute,
  onStateChange,
}: CreateWasmModuleRunnerOptions<TModule, TArgs, TResult>): WasmModuleRunner<TArgs, TResult> {
  let latestRun = 0;
  let lastArgs: TArgs | null = null;
  let disposed = false;

  async function run(...args: TArgs): Promise<TResult> {
    const runId = ++latestRun;
    lastArgs = args;

    if (!disposed) {
      onStateChange({ status: "loading", result: null, error: null });
    }

    try {
      const module = await loadModule();
      const result = await execute(module, ...args);

      if (!disposed && runId === latestRun) {
        onStateChange({ status: "ready", result, error: null });
      }

      return result;
    } catch (error: unknown) {
      if (!disposed && runId === latestRun) {
        onStateChange({ status: "error", result: null, error });
      }

      throw error;
    }
  }

  function retry(): Promise<TResult> {
    if (lastArgs === null) {
      return Promise.reject(new Error("Cannot retry before the first run."));
    }

    return run(...lastArgs);
  }

  function dispose(): void {
    disposed = true;
    latestRun += 1;
  }

  return { run, retry, dispose };
}

export interface UseWasmModuleOptions<TModule, TArgs extends unknown[], TResult> {
  loadModule: () => Promise<TModule>;
  execute: (module: TModule, ...args: TArgs) => Promise<TResult> | TResult;
}

export interface UseWasmModuleValue<TArgs extends unknown[], TResult> {
  state: WasmModuleState<TResult>;
  run: (...args: TArgs) => Promise<TResult>;
  retry: () => Promise<TResult>;
}

const IDLE_STATE: WasmModuleState<never> = {
  status: "idle",
  result: null,
  error: null,
};

/**
 * A client-only state adapter around the lazy loader and request runner.
 * Merely rendering this hook never invokes `loadModule`.
 */
export function useWasmModule<TModule, TArgs extends unknown[], TResult>({
  loadModule,
  execute,
}: UseWasmModuleOptions<TModule, TArgs, TResult>): UseWasmModuleValue<TArgs, TResult> {
  const loadModuleRef = useRef(loadModule);
  const executeRef = useRef(execute);
  const [state, setState] = useState<WasmModuleState<TResult>>(
    IDLE_STATE as WasmModuleState<TResult>,
  );

  loadModuleRef.current = loadModule;
  executeRef.current = execute;

  const runner = useMemo(
    () =>
      createWasmModuleRunner<TModule, TArgs, TResult>({
        loadModule: () => loadModuleRef.current(),
        execute: (module, ...args) => executeRef.current(module, ...args),
        onStateChange: setState,
      }),
    [],
  );

  useEffect(() => () => runner.dispose(), [runner]);

  return { state, run: runner.run, retry: runner.retry };
}
