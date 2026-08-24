import { describe, expect, it, vi } from "vitest";

import { normalizePlaygroundResult } from "../result-types";
import {
  createModuleLoader,
  createWasmModuleRunner,
  type WasmModuleState,
} from "../use-wasm-module";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("createModuleLoader", () => {
  it("keeps independent caches for separate entry points", async () => {
    const loadParse = vi.fn(async () => ({ entry: "parse" }));
    const loadHighlight = vi.fn(async () => ({ entry: "highlight" }));
    const parseLoader = createModuleLoader(loadParse);
    const highlightLoader = createModuleLoader(loadHighlight);

    const [parseFirst, parseSecond, highlight] = await Promise.all([
      parseLoader(),
      parseLoader(),
      highlightLoader(),
    ]);

    expect(parseFirst).toBe(parseSecond);
    expect(parseFirst.entry).toBe("parse");
    expect(highlight.entry).toBe("highlight");
    expect(loadParse).toHaveBeenCalledOnce();
    expect(loadHighlight).toHaveBeenCalledOnce();
  });

  it("clears a rejected load so the next call retries", async () => {
    const transientError = new Error("temporary network failure");
    const load = vi
      .fn<() => Promise<{ ready: true }>>()
      .mockRejectedValueOnce(transientError)
      .mockResolvedValueOnce({ ready: true });
    const loader = createModuleLoader(load);

    await expect(loader()).rejects.toBe(transientError);
    await expect(loader()).resolves.toEqual({ ready: true });
    expect(load).toHaveBeenCalledTimes(2);
  });
});

describe("normalizePlaygroundResult", () => {
  const diagnostic = {
    severity: "warning" as const,
    source: "highlight" as const,
    message: "opaque upstream text at an unrelated coordinate",
    line: null,
    column: null,
  };

  it("distinguishes all three resolved outcomes without rewriting diagnostics", () => {
    expect(normalizePlaygroundResult("html", [])).toEqual({
      kind: "success",
      payload: "html",
      diagnostics: [],
    });
    expect(normalizePlaygroundResult("fallback html", [diagnostic])).toEqual({
      kind: "success-with-diagnostics",
      payload: "fallback html",
      diagnostics: [diagnostic],
    });
    expect(normalizePlaygroundResult(null, [diagnostic])).toEqual({
      kind: "failure-with-diagnostics",
      payload: null,
      diagnostics: [diagnostic],
    });
    expect(normalizePlaygroundResult(null, [diagnostic]).diagnostics[0]?.message).toBe(
      diagnostic.message,
    );
  });
});

describe("createWasmModuleRunner", () => {
  it("publishes stable loading and ready states", async () => {
    const states: WasmModuleState<string>[] = [];
    const runner = createWasmModuleRunner({
      loadModule: async () => ({ prefix: "rendered" }),
      execute: async (module, input: string) => `${module.prefix}:${input}`,
      onStateChange: (state) => states.push(state),
    });

    await expect(runner.run("one")).resolves.toBe("rendered:one");
    expect(states).toEqual([
      { status: "loading", result: null, error: null },
      { status: "ready", result: "rendered:one", error: null },
    ]);
  });

  it("keeps rejected traps in the error channel and retries the last run", async () => {
    const trap = new Error("wasm trapped");
    const states: WasmModuleState<string>[] = [];
    const execute = vi
      .fn<(module: object, input: string) => Promise<string>>()
      .mockRejectedValueOnce(trap)
      .mockResolvedValueOnce("recovered");
    const runner = createWasmModuleRunner({
      loadModule: async () => ({}),
      execute,
      onStateChange: (state) => states.push(state),
    });

    await expect(runner.run("same input")).rejects.toBe(trap);
    expect(states.at(-1)).toEqual({
      status: "error",
      result: null,
      error: trap,
    });

    await expect(runner.retry()).resolves.toBe("recovered");
    expect(execute).toHaveBeenLastCalledWith({}, "same input");
    expect(states.at(-1)).toEqual({
      status: "ready",
      result: "recovered",
      error: null,
    });
  });

  it("uses last-started-run-wins when calls finish out of order", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const states: WasmModuleState<string>[] = [];
    const runner = createWasmModuleRunner({
      loadModule: async () => ({}),
      execute: (_module, input: "first" | "second") =>
        input === "first" ? first.promise : second.promise,
      onStateChange: (state) => states.push(state),
    });

    const firstRun = runner.run("first");
    const secondRun = runner.run("second");
    second.resolve("new result");
    await expect(secondRun).resolves.toBe("new result");
    first.resolve("stale result");
    await expect(firstRun).resolves.toBe("stale result");

    expect(states.at(-1)).toEqual({
      status: "ready",
      result: "new result",
      error: null,
    });
    expect(states).not.toContainEqual({
      status: "ready",
      result: "stale result",
      error: null,
    });
  });
});
