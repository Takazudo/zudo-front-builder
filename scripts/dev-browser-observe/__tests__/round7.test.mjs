import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { main } from "../cli.mjs";
import { ObserverPage, closeOwnedBrowser } from "../lib/browser.mjs";
import { runCalibration } from "../lib/calibrate.mjs";
import { liveOwnedFromRegistry, shouldRegisterOwned } from "../lib/process-owner.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { EXIT } from "../lib/run.mjs";
import { validateAs } from "../lib/schema.mjs";
import { LineSplitter } from "../lib/server.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";

const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));

describe("#1 unverifiable-birth roots reach the next-batch guard", () => {
  const spawnWallMs = Date.parse("Sat Nov 21 06:00:00 2026") + 400;
  const row = {
    type: "spawn",
    pid: 50,
    pgid: 50,
    lstart: "unavailable",
    comm: "unavailable",
    birthUnverifiable: true,
    spawnWallMs,
  };

  it("an unverifiable root is registered once its real birth is seen; a verified root is not re-registered", () => {
    expect(shouldRegisterOwned({ pid: 50, birthUnverifiable: true }, { pid: 50 })).toBe(true);
    expect(shouldRegisterOwned({ pid: 50 }, { pid: 50 })).toBe(false);
    expect(shouldRegisterOwned({ pid: 50 }, { pid: 51 })).toBe(true);
  });

  const at =
    (lstart, pgid = 50, pid = 50) =>
    () => ({ pid, pgid, lstart, comm: "zfb" });

  it("matches an unverifiable row by pid + pgid + start at or after the spawn second (darwin: no lstart slack)", () => {
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 06:00:00 2026"), "darwin")).toHaveLength(1);
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 05:59:59 2026"), "darwin")).toHaveLength(0);
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 06:00:05 2026", 77), "darwin")).toHaveLength(
      0,
    );
    expect(liveOwnedFromRegistry([row], () => null, "darwin")).toHaveLength(0);
  });

  it("on linux the 1 s procps lstart slack admits the previous second for the root row only", () => {
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 06:00:00 2026"), "linux")).toHaveLength(1);
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 05:59:59 2026"), "linux")).toHaveLength(1);
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 05:59:58 2026"), "linux")).toHaveLength(0);
    expect(liveOwnedFromRegistry([row], at("Sat Nov 21 06:00:05 2026", 77), "linux")).toHaveLength(
      0,
    );
    expect(liveOwnedFromRegistry([row], () => null, "linux")).toHaveLength(0);
    const member = { ...row, pid: 51 };
    expect(
      liveOwnedFromRegistry([member], at("Sat Nov 21 06:00:00 2026", 50, 51), "linux"),
    ).toHaveLength(1);
    expect(
      liveOwnedFromRegistry([member], at("Sat Nov 21 05:59:59 2026", 50, 51), "linux"),
    ).toHaveLength(0);
  });

  it("verified rows still require the exact birth", () => {
    const verified = { pid: 60, pgid: 60, lstart: "Sat Nov 21 06:00:00 2026", comm: "zfb" };
    for (const platform of ["darwin", "linux"])
      expect(liveOwnedFromRegistry([verified], () => ({ ...verified }), platform)).toHaveLength(1);
    for (const platform of ["darwin", "linux"])
      expect(
        liveOwnedFromRegistry(
          [verified],
          () => ({ ...verified, lstart: "Sat Nov 21 07:00:00 2026" }),
          platform,
        ),
      ).toHaveLength(0);
  });
});

describe("#2 observer-init-after-navigation is bounded", () => {
  it("nextDocInit resolves null after the timeout and removes its waiter", async () => {
    const p = new ObserverPage(null, "", () => 1n);
    expect(await p.nextDocInit(20)).toBeNull();
    expect(p.docInitWaiters).toHaveLength(0);
  });

  it("the calibration cell records a timeout instead of hanging", async () => {
    const o = new ObserverPage(null, "", () => 1n);
    o.baseUrl = "http://127.0.0.1:1";
    o.page = { goto: async () => null, waitForLoadState: async () => null };
    const records = [];
    const results = await runCalibration({
      observer: o,
      append: (r) => records.push(r),
      cells: ["observer-init-after-navigation"],
      repeats: 1,
      timeoutMs: 30,
      staleTimeoutMs: 10,
      pagePath: "/independent",
    });
    expect(records[0]).toMatchObject({
      cell: "observer-init-after-navigation",
      ok: false,
      timedOut: true,
      ms: null,
    });
    expect(results["observer-init-after-navigation"].failures).toBe(1);
  });
});

describe("#3 browser close is bounded", () => {
  it("records a timeout when context.close never settles, then still closes the browser", async () => {
    let browserClosed = false;
    const errors = await closeOwnedBrowser(
      {
        context: { close: () => new Promise(() => {}) },
        browser: { close: async () => (browserClosed = true) },
      },
      { timeoutMs: 20 },
    );
    expect(errors).toEqual([
      { step: "context.close", error: "timed out after 20 ms", timedOut: true },
    ]);
    expect(browserClosed).toBe(true);
  });
});

describe("nits", () => {
  it("#4 rejects unspecified server hosts", () => {
    const p = example("tiny-internal-a0.example.json");
    for (const host of ["0.0.0.0", "::", "[::]"]) {
      expect(semanticErrors({ ...p, server: { ...p.server, host } })[0]).toMatch(
        /must be an explicit address/,
      );
      expect(
        validateAs("profile", { ...p, server: { ...p.server, host } }).some((e) =>
          e.includes("does not match"),
        ),
      ).toBe(true);
    }
    expect(semanticErrors(p)).toEqual([]);
  });

  it.each([[["--ms", ""]], [["--ms", "  "]], [["--ms", "2147483648"]]])(
    "#5 hold-lock rejects %j",
    async (ms) => {
      expect(await main(["hold-lock", "--profile", join("nowhere", "p.json"), ...ms])).toBe(
        EXIT.usage,
      );
    },
  );

  it("#6 write-error validation uses the op's default method", () => {
    const p = example("neg-write-error.example.json");
    p.scenarios[0].ops[0] = { id: "c", op: "create-article", article: "c", inject: "write-error" };
    expect(semanticErrors(p).some((e) => /write-error" requires writeMethod/.test(e))).toBe(true);
    p.scenarios[0].ops[0] = { id: "t", op: "edit-title", article: "a", inject: "write-error" };
    expect(semanticErrors(p).some((e) => /write-error" requires writeMethod/.test(e))).toBe(false);
  });

  it("#7 a multi-byte character split across chunks is decoded intact", () => {
    const s = new LineSplitter();
    const bytes = Buffer.from("→ ready on http://127.0.0.1:1\n");
    expect(s.push(bytes.subarray(0, 1))).toEqual([]);
    expect(s.push(bytes.subarray(1))).toEqual(["→ ready on http://127.0.0.1:1"]);
  });
});
