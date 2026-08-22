import { describe, expect, it } from "vitest";

import {
  createWasmApi,
  ZfbMdWasmTrapError,
  ZfbMdWasmTrapRecoveryLimitError,
} from "../src/runtime.js";

const FAKE_MODULE = {} as WebAssembly.Module;
const FAKE_BYTES = new ArrayBuffer(8);

function fakeGlue(initSync: () => void = () => undefined, forceTrap: () => void = () => undefined) {
  return {
    initSync,
    compile: () => JSON.stringify({ code: "", frontmatter: null, diagnostics: [] }),
    renderHtml: () => JSON.stringify({ html: "", frontmatter: null, diagnostics: [] }),
    parseToAst: () => JSON.stringify({ ast: { type: "root", children: [] }, diagnostics: [] }),
    highlightCode: () => JSON.stringify({ html: "", diagnostics: [] }),
    version: () => "1.2.3-test",
    __forceTrapForTests: forceTrap,
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

const ENTRY_MATRIX = [
  {
    name: "root",
    invoke: (api: ReturnType<typeof createWasmApi>) => api.compile("# root"),
  },
  {
    name: "highlight",
    invoke: (api: ReturnType<typeof createWasmApi>) =>
      api.highlightCode("const root = 1;", { language: "javascript" }),
  },
  {
    name: "render",
    invoke: (api: ReturnType<typeof createWasmApi>) => api.renderHtml("# render"),
  },
  {
    name: "parse",
    invoke: (api: ReturnType<typeof createWasmApi>) => api.parseToAst("# parse"),
  },
] as const;

function makeMatrixApi(name: string, config: Partial<Parameters<typeof createWasmApi>[0]> = {}) {
  return createWasmApi({
    glueUrl: new URL(`https://example.test/${name}.mjs`),
    loadWasmBytes: async () => FAKE_BYTES,
    compileWasm: async () => FAKE_MODULE,
    importGlue: async () => fakeGlue(),
    ...config,
  });
}

describe("all public entry runtime state machines", () => {
  for (const entry of ENTRY_MATRIX) {
    it(`${entry.name} single-flights concurrent capability calls`, async () => {
      const firstLoad = deferred<ArrayBuffer>();
      let loadAttempts = 0;
      let compileAttempts = 0;
      let importAttempts = 0;
      let initAttempts = 0;
      const api = makeMatrixApi(entry.name, {
        loadWasmBytes: () => {
          loadAttempts += 1;
          return firstLoad.promise;
        },
        compileWasm: async () => {
          compileAttempts += 1;
          return FAKE_MODULE;
        },
        importGlue: async () => {
          importAttempts += 1;
          return fakeGlue(() => {
            initAttempts += 1;
          });
        },
      });

      const calls = [entry.invoke(api), entry.invoke(api), api.init()];
      await Promise.resolve();
      expect(loadAttempts).toBe(1);
      expect(compileAttempts).toBe(0);
      expect(importAttempts).toBe(1);
      firstLoad.resolve(FAKE_BYTES);
      await Promise.all(calls);
      expect(compileAttempts).toBe(1);
      expect(initAttempts).toBe(1);
      expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 1,
        freshInstanceStarts: 1,
        glueImportAttempts: 1,
        currentGeneration: 0,
      });
    });

    it(`${entry.name} retries transient bytes, glue, and initSync failures`, async () => {
      const byteError = new Error(`${entry.name} byte retry`);
      let byteLoads = 0;
      const byteApi = makeMatrixApi(`${entry.name}-bytes`, {
        loadWasmBytes: async () => {
          byteLoads += 1;
          if (byteLoads === 1) throw byteError;
          return FAKE_BYTES;
        },
      });
      await expect(entry.invoke(byteApi)).rejects.toBe(byteError);
      await expect(entry.invoke(byteApi)).resolves.toBeDefined();
      expect(byteLoads).toBe(2);
      expect(byteApi.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 2,
        freshInstanceStarts: 2,
      });

      const glueError = new Error(`${entry.name} glue retry`);
      let glueImports = 0;
      let glueCompiles = 0;
      const glueApi = makeMatrixApi(`${entry.name}-glue`, {
        compileWasm: async () => {
          glueCompiles += 1;
          return FAKE_MODULE;
        },
        importGlue: async () => {
          glueImports += 1;
          if (glueImports === 1) throw glueError;
          return fakeGlue();
        },
      });
      await expect(entry.invoke(glueApi)).rejects.toBe(glueError);
      await expect(entry.invoke(glueApi)).resolves.toBeDefined();
      expect(glueCompiles).toBe(1);
      expect(glueImports).toBe(2);
      expect(glueApi.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 1,
        freshInstanceStarts: 2,
        glueImportAttempts: 2,
      });

      const initError = new Error(`${entry.name} initSync retry`);
      let initImports = 0;
      let initCompiles = 0;
      const initApi = makeMatrixApi(`${entry.name}-init`, {
        compileWasm: async () => {
          initCompiles += 1;
          return FAKE_MODULE;
        },
        importGlue: async () => {
          initImports += 1;
          return fakeGlue(() => {
            if (initImports === 1) throw initError;
          });
        },
      });
      await expect(entry.invoke(initApi)).rejects.toBe(initError);
      await expect(entry.invoke(initApi)).resolves.toBeDefined();
      expect(initCompiles).toBe(1);
      expect(initImports).toBe(2);
      expect(initApi.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 1,
        freshInstanceStarts: 2,
        glueImportAttempts: 2,
      });
    });

    it(`${entry.name} replaces one trapped generation and the next call succeeds`, async () => {
      let imports = 0;
      const api = makeMatrixApi(`${entry.name}-trap`, {
        importGlue: async () => {
          const instanceNumber = ++imports;
          return fakeGlue(undefined, () => {
            if (instanceNumber === 1) throw new WebAssembly.RuntimeError("one-shot trap");
          });
        },
      });
      await expect(api.__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);
      await expect(entry.invoke(api)).resolves.toBeDefined();
      expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 1,
        currentGeneration: 1,
        freshInstanceStarts: 2,
        trapRecoveriesStarted: 1,
        terminal: false,
      });
    });

    it(`${entry.name} gives concurrent trap reporters one replacement`, async () => {
      let imports = 0;
      const api = makeMatrixApi(`${entry.name}-reporters`, {
        importGlue: async () => {
          const instanceNumber = ++imports;
          return fakeGlue(undefined, () => {
            if (instanceNumber === 1) throw new WebAssembly.RuntimeError("concurrent trap");
          });
        },
      });
      await api.init();
      const results = await Promise.allSettled([
        api.__forceTrapForTests(),
        api.__forceTrapForTests(),
        api.__forceTrapForTests(),
      ]);
      expect(results).toHaveLength(3);
      for (const result of results) {
        expect(result.status).toBe("rejected");
        if (result.status === "rejected") {
          expect(result.reason).toBeInstanceOf(ZfbMdWasmTrapError);
        }
      }
      expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
        currentGeneration: 1,
        freshInstanceStarts: 2,
        glueImportAttempts: 2,
        trapRecoveriesStarted: 1,
        terminal: false,
      });
      await expect(entry.invoke(api)).resolves.toBeDefined();
    });

    it(`${entry.name} stops after exactly sixteen successful recoveries`, async () => {
      const api = makeMatrixApi(`${entry.name}-bound`, {
        importGlue: async () =>
          fakeGlue(undefined, () => {
            throw new WebAssembly.RuntimeError("repeated trap");
          }),
      });
      await api.init();
      for (let recovery = 0; recovery < 16; recovery += 1) {
        await expect(api.__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);
      }
      expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
        compiledModuleLoads: 1,
        currentGeneration: 16,
        freshInstanceStarts: 17,
        trapRecoveriesStarted: 16,
        maxTrapRecoveries: 16,
        terminal: false,
      });
      await expect(api.__forceTrapForTests()).rejects.toBeInstanceOf(
        ZfbMdWasmTrapRecoveryLimitError,
      );
      expect(api.__getTrapRecoveryStateForTests()).toMatchObject({
        currentGeneration: 16,
        trapRecoveriesStarted: 16,
        terminal: true,
      });
      await expect(entry.invoke(api)).rejects.toBeInstanceOf(ZfbMdWasmTrapRecoveryLimitError);
    });
  }

  it("isolates simultaneous root/highlight/render/parse generations and terminal state", async () => {
    const apis = new Map<string, ReturnType<typeof createWasmApi>>();
    for (const entry of ENTRY_MATRIX) {
      const api = makeMatrixApi(`isolated-${entry.name}`, {
        importGlue: async () =>
          fakeGlue(undefined, () => {
            if (entry.name === "root") throw new WebAssembly.RuntimeError("root-only trap");
          }),
      });
      apis.set(entry.name, api);
    }
    await Promise.all([...apis.values()].map((api) => api.init()));
    for (const [name, api] of apis) {
      expect(api.__getTrapRecoveryStateForTests(), name).toMatchObject({
        compiledModuleLoads: 1,
        currentGeneration: 0,
        freshInstanceStarts: 1,
        terminal: false,
      });
    }

    const root = apis.get("root")!;
    await expect(root.__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);
    for (let recovery = 1; recovery < 16; recovery += 1) {
      await expect(root.__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);
    }
    await expect(root.__forceTrapForTests()).rejects.toBeInstanceOf(
      ZfbMdWasmTrapRecoveryLimitError,
    );
    expect(root.__getTrapRecoveryStateForTests()).toMatchObject({
      currentGeneration: 16,
      freshInstanceStarts: 17,
      terminal: true,
    });

    for (const entry of ENTRY_MATRIX.filter(({ name }) => name !== "root")) {
      const api = apis.get(entry.name)!;
      await expect(entry.invoke(api)).resolves.toBeDefined();
      expect(api.__getTrapRecoveryStateForTests(), entry.name).toMatchObject({
        compiledModuleLoads: 1,
        currentGeneration: 0,
        freshInstanceStarts: 1,
        terminal: false,
      });
    }
  });
});

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
