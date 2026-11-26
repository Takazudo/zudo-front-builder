import { getEventListeners } from "node:events";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setImmediate } from "node:timers/promises";
import { describe, expect, it, vi } from "vitest";
import { BrowserTiming } from "../lib/browser-timing.mjs";
import { DEFAULT_BROWSER, OBSERVER_PATH, launchOwnedBrowser } from "../lib/browser.mjs";
import { ControllerTiming } from "../lib/controller-timing.mjs";
import { HARNESS_DIR, sha256, sha256File } from "../lib/util.mjs";

function fixture({
  contextError = null,
  closeError = null,
  selected = join(HARNESS_DIR, "cli.mjs"),
  library = join(HARNESS_DIR, "lib", "util.mjs"),
} = {}) {
  const events = [];
  const page = {
    on: (name) => events.push(name),
  };
  const context = {
    exposeBinding: async (name) => events.push(name),
    addInitScript: async ({ content }) => {
      expect(content).toBe(readFileSync(OBSERVER_PATH, "utf8"));
      events.push("init");
    },
    newPage: async () => {
      events.push("page");
      return page;
    },
  };
  const browser = {
    newContext: async (opts) => {
      events.push("context");
      expect(opts.serviceWorkers).toBe("block");
      if (contextError) throw contextError;
      return context;
    },
    version: () => "browser-version",
    close: async () => {
      events.push("close");
      if (closeError) throw closeError;
    },
  };
  const chromium = {
    executablePath: () => {
      events.push("lookup");
      return library;
    },
    launch: async (opts) => {
      events.push("launch");
      expect(opts).toEqual({
        headless: DEFAULT_BROWSER.headless,
        args: DEFAULT_BROWSER.args,
        executablePath: selected,
        timeout: 1234,
      });
      return browser;
    },
  };
  const config = {
    diagnosticExecutable: { path: selected, sha256: sha256File(selected), timeoutMs: 1234 },
  };
  return { selected, library, events, browser, context, chromium, config };
}

describe("browser startup timing", () => {
  it("retains stable associations without keeping a synchronous controller parent across an await", async () => {
    const timing = new BrowserTiming();
    const controller = new ControllerTiming();
    let resolve;
    const deferred = new Promise((callback) => {
      resolve = callback;
    });
    const result = timing.asyncSpan("browser.await", null, (id) => {
      timing.syncSpan("browser.prefix", id, {}, () => {});
      return deferred;
    });
    expect(result).toBe(deferred);
    const interim = timing.snapshot();
    expect(interim).toMatchObject({
      pending: 1,
      incomplete: true,
      asyncSpans: [{ id: 0, parentId: null, returnKind: "promise", settledObservedNs: null }],
      syncSpans: [{ id: 1, parentId: 0, outcome: "returned" }],
    });
    await setImmediate();
    controller.stage("sample.record", {}, () => {});
    const controllerStart = BigInt(controller.report.stages[0].startNs);
    expect(controller.report.stages[0].parentId).toBeNull();
    resolve(7);
    await expect(result).resolves.toBe(7);
    const final = timing.snapshot();
    expect(final).toMatchObject({
      incomplete: false,
      pending: 0,
      asyncSpans: [{ outcome: "fulfilled" }],
    });
    expect(BigInt(final.asyncSpans[0].returnNs)).toBeLessThan(controllerStart);
    expect(controllerStart).toBeLessThan(BigInt(final.asyncSpans[0].settledObservedNs));
    expect(interim.asyncSpans[0].outcome).toBe("pending");
  });

  it("marks cap exhaustion and unsupported settlement with no interference", () => {
    const timing = new BrowserTiming({ cap: 3 });
    const thenable = {
      get then() {
        throw new Error("the observer must not inspect arbitrary thenables");
      },
    };
    expect(timing.asyncSpan("test.thenable", null, () => thenable)).toBe(thenable);
    timing.mark("test.boundary");
    timing.syncSpan("test.prefix", null, {}, () => {});
    expect(timing.asyncSpan("test.dropped", null, () => "same value")).toBe("same value");
    expect(timing.snapshot()).toMatchObject({
      cap: 3,
      unobserved: 1,
      pending: 0,
      dropped: 1,
      incomplete: true,
      asyncSpans: [{ outcome: "settlement-unobserved", settledObservedNs: null }],
    });
  });

  it("preserves a Promise when attaching a settlement observation throws", () => {
    class OwnSpecies extends Promise {
      static get [Symbol.species]() {
        throw new Error("observer attach failed");
      }
    }
    const timing = new BrowserTiming();
    const promise = new OwnSpecies((resolve) => resolve(1));
    expect(timing.asyncSpan("test.species", null, () => promise)).toBe(promise);
    expect(timing.snapshot()).toMatchObject({
      unobserved: 1,
      asyncSpans: [{ returnKind: "promise", outcome: "settlement-unobserved" }],
    });
  });

  it("retains a synchronous throw's completed prefix and exact exception", () => {
    const timing = new BrowserTiming();
    const failure = new Error("original");
    expect(() =>
      timing.asyncSpan("browser.rejected-before-return", null, (id) =>
        timing.syncSpan("browser.sync", id, {}, () => {
          throw failure;
        }),
      ),
    ).toThrow(failure);
    expect(timing.snapshot()).toMatchObject({
      pending: 0,
      asyncSpans: [
        {
          enterNs: expect.any(String),
          returnNs: null,
          throwObservedNs: expect.any(String),
          settledObservedNs: null,
          outcome: "threw",
        },
      ],
      syncSpans: [{ startNs: expect.any(String), endNs: expect.any(String), outcome: "threw" }],
    });
  });

  it("separates selected, library and observer reads from digest work in order", async () => {
    const f = fixture();
    const timing = new BrowserTiming();
    const result = await timing.asyncSpan("browser.initialize", null, (parentId) =>
      launchOwnedBrowser(f.config, { chromiumImpl: f.chromium, timing, parentId }),
    );
    expect(result.browser).toBe(f.browser);
    expect(result.context).toBe(f.context);
    const report = timing.snapshot();
    expect(report.incomplete).toBe(false);
    expect(result.identity).toMatchObject({
      launchedExecutablePath: f.selected,
      launchedExecutableSha256: sha256File(f.selected),
      executablePath: f.library,
      executableSha256: sha256File(f.library),
    });
    expect(f.events).toEqual([
      "lookup",
      "launch",
      "context",
      "__dboNotify",
      "__dboPing",
      "__dboDocInit",
      "init",
      "page",
      "framenavigated",
      "request",
      "websocket",
    ]);
    const files = [...report.asyncSpans, ...report.syncSpans]
      .filter((row) => ["browser.file.read", "browser.file.digest"].includes(row.name))
      .sort((a, b) => a.id - b.id);
    expect(files.map(({ name, role, path }) => [name, role, path])).toEqual(
      [
        ["selected-launched", f.selected],
        ["library-default", f.library],
        ["observer-source", OBSERVER_PATH],
      ].flatMap(([role, path]) => [
        ["browser.file.read", role, path],
        ["browser.file.digest", role, path],
      ]),
    );
    for (const row of files) {
      expect(row.parentId).toBe(0);
      expect(row.bytes).toBe(readFileSync(row.path).length);
      if (row.name === "browser.file.digest") expect(row.digest).toBe(sha256File(row.path));
    }
    for (const index of [0, 2]) {
      expect(files[index]).toMatchObject({ returnKind: "promise", outcome: "fulfilled" });
      expect(BigInt(files[index].returnNs)).toBeLessThanOrEqual(
        BigInt(files[index].settledObservedNs),
      );
      expect(BigInt(files[index].settledObservedNs)).toBeLessThanOrEqual(
        BigInt(files[index + 1].startNs),
      );
    }
    expect(files[4].outcome).toBe("returned");
    const launch = report.asyncSpans.find((row) => row.name === "browser.chromium.launch");
    expect(BigInt(files[1].endNs)).toBeLessThanOrEqual(BigInt(launch.enterNs));
    const context = report.asyncSpans.find((row) => row.name === "browser.context.create");
    expect(BigInt(context.settledObservedNs)).toBeLessThanOrEqual(BigInt(files[2].enterNs));
    const install = report.asyncSpans.find((row) => row.name === "browser.observer.install");
    expect(
      report.asyncSpans.filter((row) => row.parentId === install.id).map(({ name }) => name),
    ).toEqual([
      "browser.observer.binding.notify",
      "browser.observer.binding.ping",
      "browser.observer.binding.doc-init",
      "browser.observer.init-script",
      "browser.observer.page.create",
    ]);
    expect(report.syncSpans.find((row) => row.name === "browser.observer.listeners").parentId).toBe(
      install.id,
    );
    const create = report.syncSpans.find((row) => row.name === "browser.observer.create");
    expect(
      report.syncSpans.find((row) => row.name === "browser.observer.source-read").parentId,
    ).toBe(create.id);
  });

  it("records the digest and rejects its mismatch before launching", async () => {
    const f = fixture();
    const timing = new BrowserTiming();
    f.config.diagnosticExecutable.sha256 = "0".repeat(64);
    await expect(
      timing.asyncSpan("browser.initialize", null, (parentId) =>
        launchOwnedBrowser(f.config, { chromiumImpl: f.chromium, timing, parentId }),
      ),
    ).rejects.toThrow(/digest differs/);
    expect(f.events).toEqual(["lookup"]);
    const report = timing.snapshot();
    expect(report.asyncSpans).toMatchObject([
      { name: "browser.initialize", outcome: "rejected" },
      { name: "browser.file.read", role: "selected-launched", outcome: "fulfilled" },
    ]);
    expect(report.syncSpans.at(-1)).toMatchObject({
      name: "browser.file.digest",
      role: "selected-launched",
      digest: sha256File(f.selected),
    });
  });

  it.each([false, true])(
    "records setup rejection and original cleanup result with close failure %s",
    async (closeFailure) => {
      const contextError = new Error("original context error");
      const closeError = closeFailure ? new Error("close failure") : null;
      const f = fixture({ contextError, closeError });
      const timing = new BrowserTiming();
      await expect(
        timing.asyncSpan("browser.initialize", null, (parentId) =>
          launchOwnedBrowser(f.config, { chromiumImpl: f.chromium, timing, parentId }),
        ),
      ).rejects.toBe(contextError);
      expect(f.events).toEqual(["lookup", "launch", "context", "close"]);
      expect(timing.snapshot()).toMatchObject({
        incomplete: false,
        asyncSpans: [
          {
            name: "browser.initialize",
            outcome: "rejected",
            settledObservedNs: expect.any(String),
          },
          { name: "browser.file.read", role: "selected-launched", outcome: "fulfilled" },
          { name: "browser.chromium.launch", outcome: "fulfilled" },
          {
            name: "browser.context.create",
            outcome: "rejected",
            settledObservedNs: expect.any(String),
          },
          { name: "browser.cleanup.close", outcome: closeFailure ? "rejected" : "fulfilled" },
        ],
      });
      if (closeFailure) {
        expect(contextError.partialBrowser.browser).toBe(f.browser);
        expect(contextError.browserCleanupError).toMatch(/close failure/);
      } else {
        expect(contextError.partialBrowser).toBeUndefined();
      }
    },
  );
});

describe.each([false, true])("executable identity with browser timing %s", (enabled) => {
  const options = (f) => ({
    chromiumImpl: f.chromium,
    timing: enabled ? new BrowserTiming() : null,
  });

  it("reads fresh bytes twice even when both roles use the same path", async () => {
    const dir = mkdtempSync(join(tmpdir(), "dbo-executable-"));
    const path = join(dir, "browser");
    try {
      writeFileSync(path, "selected");
      const f = fixture({ selected: path, library: path });
      const opts = options(f);
      f.browser.newContext = async () => {
        writeFileSync(path, "library after launch");
        return f.context;
      };
      const { identity } = await launchOwnedBrowser(f.config, opts);
      expect(identity.launchedExecutableSha256).toBe(sha256("selected"));
      expect(identity.executableSha256).toBe(sha256("library after launch"));
      f.events.length = 0;
      await expect(launchOwnedBrowser(f.config, opts)).rejects.toThrow(/digest differs/);
      expect(f.events).toEqual(["lookup"]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("reads only the default executable for ordinary launches and releases its abort listener", async () => {
    const f = fixture();
    const opts = options(f);
    f.chromium.launch = async () => f.browser;
    const abort = new AbortController();
    const readExecutable = vi.fn(readFile);
    const { identity } = await launchOwnedBrowser(
      {},
      { ...opts, signal: abort.signal, readExecutable },
    );
    expect(readExecutable.mock.calls).toEqual([[f.library, { signal: abort.signal }]]);
    expect(identity.executableSha256).toBe(sha256File(f.library));
    expect(identity.launchedExecutableSha256).toBeUndefined();
    expect(getEventListeners(abort.signal, "abort")).toHaveLength(0);
    abort.abort();
    if (enabled)
      expect(
        opts.timing.snapshot().asyncSpans.find((row) => row.role === "library-default")
          .cancelRequestedNs,
      ).toBeNull();
  });

  it.each(["selected-launched", "library-default"])(
    "keeps ownership of the pending %s read through late fulfillment and abort",
    async (role) => {
      const f = fixture();
      const opts = options(f);
      const abort = new AbortController();
      const entered = Promise.withResolvers();
      const gate = Promise.withResolvers();
      const target = role === "selected-launched" ? f.selected : f.library;
      const states = [];
      const startup = launchOwnedBrowser(f.config, {
        ...opts,
        signal: abort.signal,
        onReadState: (details, state) => states.push([details.role, state]),
        readExecutable: (path, { signal }) => {
          expect(signal).toBe(abort.signal);
          if (path !== target) return readFile(path, { signal });
          entered.resolve();
          return gate.promise;
        },
      });
      const rejection = expect(startup).rejects.toThrow("cancel during read");
      await entered.promise;
      abort.abort(new Error("cancel during read"));
      await setImmediate();
      expect(states.at(-1)).toEqual([role, "pending"]);
      expect(f.events).toEqual(
        role === "selected-launched" ? ["lookup"] : ["lookup", "launch", "context"],
      );
      if (enabled)
        expect(opts.timing.snapshot().asyncSpans.at(-1)).toMatchObject({
          role,
          cancelRequestedNs: expect.any(String),
          outcome: "pending",
          settledObservedNs: null,
        });
      gate.resolve(readFileSync(target));
      await rejection;
      expect(states.at(-1)).toEqual([role, "fulfilled"]);
      expect(f.events).toEqual(
        role === "selected-launched" ? ["lookup"] : ["lookup", "launch", "context", "close"],
      );
      expect(getEventListeners(abort.signal, "abort")).toHaveLength(0);
      if (enabled) {
        const { asyncSpans, syncSpans, pending } = opts.timing.snapshot();
        expect(pending).toBe(0);
        expect(asyncSpans.find((row) => row.role === role)).toMatchObject({
          outcome: "fulfilled",
          settledObservedNs: expect.any(String),
        });
        expect(
          syncSpans.some((row) => row.name === "browser.file.digest" && row.role === role),
        ).toBe(false);
      }
    },
  );

  it.each(["selected-launched", "library-default"])(
    "propagates a rejected %s read and owns incomplete close",
    async (role) => {
      const failure = new Error("read rejected");
      const f = fixture({ closeError: new Error("browser still open") });
      const opts = options(f);
      const target = role === "selected-launched" ? f.selected : f.library;
      await expect(
        launchOwnedBrowser(f.config, {
          ...opts,
          readExecutable: (path, options) =>
            path === target ? Promise.reject(failure) : readFile(path, options),
        }),
      ).rejects.toBe(failure);
      if (role === "selected-launched") {
        expect(f.events).toEqual(["lookup"]);
        expect(failure.partialBrowser).toBeUndefined();
      } else {
        expect(f.events).toEqual(["lookup", "launch", "context", "close"]);
        expect(failure.partialBrowser.browser).toBe(f.browser);
        expect(failure.browserCleanupError).toContain("browser still open");
      }
      if (enabled)
        expect(opts.timing.snapshot().asyncSpans.find((row) => row.role === role)).toMatchObject({
          role,
          outcome: "rejected",
          settledObservedNs: expect.any(String),
        });
    },
  );

  it("cancels before submission when admission bookkeeping sees an abort", async () => {
    const f = fixture();
    const abort = new AbortController();
    const readExecutable = vi.fn(readFile);
    const states = [];
    await expect(
      launchOwnedBrowser(f.config, {
        ...options(f),
        signal: abort.signal,
        readExecutable,
        onReadState(_details, state) {
          states.push(state);
          if (state === "pending") abort.abort(new Error("stopped before read"));
        },
      }),
    ).rejects.toThrow("stopped before read");
    expect(readExecutable).not.toHaveBeenCalled();
    expect(states).toEqual(["pending", "cancelled-before-read"]);
  });

  it("returns partial ownership even when the abort reason is immutable", async () => {
    const f = fixture({ closeError: new Error("failed close") });
    const abort = new AbortController();
    const reason = Object.freeze(new Error("stop setup"));
    const opts = {
      ...options(f),
      signal: abort.signal,
      observerFactory: () => ({
        install: () => abort.abort(reason),
      }),
    };
    await expect(launchOwnedBrowser(f.config, opts)).rejects.toMatchObject({
      cause: reason,
      partialBrowser: { browser: f.browser },
      browserCleanupError: "Error: failed close",
    });
  });

  it("skips an already cancelled read and suppresses post-install readiness on cancellation", async () => {
    const f = fixture();
    const abort = new AbortController();
    const readExecutable = vi.fn(readFile);
    const opts = { ...options(f), readExecutable, signal: abort.signal };
    const install = () => abort.abort(new Error("stopped"));
    await expect(
      launchOwnedBrowser(f.config, { ...opts, observerFactory: () => ({ install }) }),
    ).rejects.toThrow("stopped");
    expect(f.events).toEqual(["lookup", "launch", "context", "close"]);
    readExecutable.mockClear();
    await expect(launchOwnedBrowser(f.config, opts)).rejects.toThrow("stopped");
    expect(readExecutable).not.toHaveBeenCalled();
  });
});
