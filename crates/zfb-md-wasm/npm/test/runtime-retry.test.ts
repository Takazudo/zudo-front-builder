import { describe, expect, it } from "vitest";

import { createWasmApi } from "../src/runtime.js";

const FAKE_MODULE = {} as WebAssembly.Module;
const FAKE_BYTES = new ArrayBuffer(8);

function fakeGlue(initSync: () => void = () => undefined) {
  return {
    initSync,
    compile: () => JSON.stringify({ code: "", frontmatter: null, diagnostics: [] }),
    renderHtml: () => JSON.stringify({ html: "", frontmatter: null, diagnostics: [] }),
    highlightCode: () => JSON.stringify({ html: "", diagnostics: [] }),
    version: () => "1.2.3-test",
    __forceTrapForTests: () => undefined,
  };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, reject, resolve };
}

function expectTrapStateUntouched(api: ReturnType<typeof createWasmApi>) {
  expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
    currentGeneration: 0,
    trapRecoveriesStarted: 0,
    terminal: false,
  });
}

describe("transient initialization retry", () => {
  it("retries a one-shot wasm resource rejection with a fresh load and compile", async () => {
    const transientError = new Error("temporary wasm read failure");
    let loadAttempts = 0;
    let compileAttempts = 0;
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: async () => {
        loadAttempts += 1;
        if (loadAttempts === 1) {
          throw transientError;
        }
        return FAKE_BYTES;
      },
      compileWasm: async () => {
        compileAttempts += 1;
        return FAKE_MODULE;
      },
      importGlue: async () => fakeGlue(),
    });

    await expect(api.init()).rejects.toBe(transientError);
    await expect(api.version()).resolves.toBe("1.2.3-test");

    expect(loadAttempts).toBe(2);
    expect(compileAttempts).toBe(1);
    expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
      compiledModuleLoads: 2,
      freshInstanceStarts: 2,
      glueImportAttempts: 2,
    });
    expectTrapStateUntouched(api);
  });

  it("retries a one-shot compile rejection with a fresh load and compile", async () => {
    const transientError = new Error("temporary compile failure");
    let loadAttempts = 0;
    let compileAttempts = 0;
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: async () => {
        loadAttempts += 1;
        return FAKE_BYTES;
      },
      compileWasm: async () => {
        compileAttempts += 1;
        if (compileAttempts === 1) {
          throw transientError;
        }
        return FAKE_MODULE;
      },
      importGlue: async () => fakeGlue(),
    });

    await expect(api.init()).rejects.toBe(transientError);
    await expect(api.version()).resolves.toBe("1.2.3-test");

    expect(loadAttempts).toBe(2);
    expect(compileAttempts).toBe(2);
    expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
      compiledModuleLoads: 2,
      freshInstanceStarts: 2,
      glueImportAttempts: 2,
    });
    expectTrapStateUntouched(api);
  });

  it("retries a one-shot glue import rejection without recompiling", async () => {
    const transientError = new Error("temporary glue import failure");
    let loadAttempts = 0;
    let compileAttempts = 0;
    let importAttempts = 0;
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: async () => {
        loadAttempts += 1;
        return FAKE_BYTES;
      },
      compileWasm: async () => {
        compileAttempts += 1;
        return FAKE_MODULE;
      },
      importGlue: async () => {
        importAttempts += 1;
        if (importAttempts === 1) {
          throw transientError;
        }
        return fakeGlue();
      },
    });

    await expect(api.init()).rejects.toBe(transientError);
    await expect(api.version()).resolves.toBe("1.2.3-test");

    expect(loadAttempts).toBe(1);
    expect(compileAttempts).toBe(1);
    expect(importAttempts).toBe(2);
    expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
      compiledModuleLoads: 1,
      freshInstanceStarts: 2,
      glueImportAttempts: 2,
    });
    expectTrapStateUntouched(api);
  });

  it("retries initSync with a new import nonce and the successful compiled module", async () => {
    const transientError = new Error("temporary initSync failure");
    let compileAttempts = 0;
    const importSpecifiers: string[] = [];
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: async () => FAKE_BYTES,
      compileWasm: async () => {
        compileAttempts += 1;
        return FAKE_MODULE;
      },
      importGlue: async (specifier) => {
        importSpecifiers.push(specifier);
        return fakeGlue(() => {
          if (importSpecifiers.length === 1) {
            throw transientError;
          }
        });
      },
    });

    await expect(api.init()).rejects.toBe(transientError);
    await expect(api.version()).resolves.toBe("1.2.3-test");

    expect(compileAttempts).toBe(1);
    expect(importSpecifiers).toHaveLength(2);
    expect(importSpecifiers[0]).not.toBe(importSpecifiers[1]);
    expect(
      importSpecifiers.map((specifier) => new URL(specifier).searchParams.get("zfbMdWasmGen")),
    ).toEqual(["0", "0"]);
    expect(
      importSpecifiers.map((specifier) => new URL(specifier).searchParams.get("zfbMdWasmAttempt")),
    ).toEqual(["1", "2"]);
    expectTrapStateUntouched(api);
  });

  it("single-flights a shared failure and the following concurrent retry", async () => {
    const firstLoad = deferred<ArrayBuffer>();
    const retryLoad = deferred<ArrayBuffer>();
    const transientError = new Error("temporary shared load failure");
    let loadAttempts = 0;
    let compileAttempts = 0;
    let importAttempts = 0;
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: () => {
        loadAttempts += 1;
        return loadAttempts === 1 ? firstLoad.promise : retryLoad.promise;
      },
      compileWasm: async () => {
        compileAttempts += 1;
        return FAKE_MODULE;
      },
      importGlue: async () => {
        importAttempts += 1;
        return fakeGlue();
      },
    });

    const failedInit = api.init();
    const failedVersion = api.version();
    await Promise.resolve();
    expect(loadAttempts).toBe(1);
    expect(importAttempts).toBe(1);

    firstLoad.reject(transientError);
    const failedResults = await Promise.allSettled([failedInit, failedVersion]);
    expect(failedResults).toEqual([
      { status: "rejected", reason: transientError },
      { status: "rejected", reason: transientError },
    ]);

    const retryInit = api.init();
    const retryVersion = api.version();
    await Promise.resolve();
    expect(loadAttempts).toBe(2);
    expect(importAttempts).toBe(2);
    retryLoad.resolve(FAKE_BYTES);
    await expect(retryInit).resolves.toBeUndefined();
    await expect(retryVersion).resolves.toBe("1.2.3-test");

    expect(compileAttempts).toBe(1);
    expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
      compiledModuleLoads: 2,
      freshInstanceStarts: 2,
      glueImportAttempts: 2,
    });
    expectTrapStateUntouched(api);
  });

  it("does not let stale rejection cleanup clear a newer installed attempt", async () => {
    const firstLoad = deferred<ArrayBuffer>();
    const retryLoad = deferred<ArrayBuffer>();
    const transientError = new Error("controlled stale cleanup");
    let loadAttempts = 0;
    let importAttempts = 0;
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/glue.mjs"),
      loadWasmBytes: () => {
        loadAttempts += 1;
        return loadAttempts === 1 ? firstLoad.promise : retryLoad.promise;
      },
      compileWasm: async () => FAKE_MODULE,
      importGlue: async () => {
        importAttempts += 1;
        return fakeGlue();
      },
    });

    // Capture the runtime's rejection-cleanup callbacks and replay them after
    // the retry is installed. This deterministically models callbacks from an
    // older promise running late; without the exact-promise guards, the replay
    // would clear the pending retry and the second caller would start a third
    // load/import attempt.
    type RejectionHandler = (reason: unknown) => unknown;
    const promisePrototype = Promise.prototype as unknown as {
      catch(onRejected?: RejectionHandler | null): Promise<unknown>;
    };
    const originalCatch = promisePrototype.catch;
    const staleCleanups: Array<() => void> = [];
    promisePrototype.catch = function (this: Promise<unknown>, onRejected) {
      return originalCatch.call(this, (reason: unknown) => {
        if (!onRejected) {
          throw reason;
        }
        const result = onRejected(reason);
        staleCleanups.push(() => {
          void onRejected(reason);
        });
        return result;
      });
    };

    try {
      const observedFailure = api.init();
      firstLoad.reject(transientError);
      const observedError = await observedFailure.then(
        () => undefined,
        (reason: unknown) => reason,
      );
      expect(observedError).toBe(transientError);
      expect(staleCleanups).toHaveLength(2);

      const retryInit = api.init();
      await Promise.resolve();
      expect(loadAttempts).toBe(2);
      expect(importAttempts).toBe(2);

      for (const runStaleCleanup of staleCleanups) {
        runStaleCleanup();
      }
      const retryVersion = api.version();
      await Promise.resolve();
      expect(loadAttempts).toBe(2);
      expect(importAttempts).toBe(2);

      retryLoad.resolve(FAKE_BYTES);
      await Promise.all([retryInit, retryVersion]);
    } finally {
      promisePrototype.catch = originalCatch;
    }

    expect(loadAttempts).toBe(2);
    expect(importAttempts).toBe(2);
    expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
      compiledModuleLoads: 2,
      freshInstanceStarts: 2,
      glueImportAttempts: 2,
    });
    expectTrapStateUntouched(api);
  });
});
