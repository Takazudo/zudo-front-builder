import { describe, it, expect } from "vitest";
import { linuxBirth, parseLinuxStat } from "../lib/linux-birth.mjs";
import { threadCount, threadCountAsync } from "../lib/mac-process-adapter.mjs";
import { sameBirth, liveOwnedFromRegistry, stopOwned } from "../lib/process-owner.mjs";

const ticks = "100719925474099300";
const stat = `42 (odd ) (name\n)) S 1 42 ${Array(16).fill("0").join(" ")} ${ticks} 0`;
const bootId = "00000000-1111-2222-3333-444444444444";
const record = linuxBirth(42, (path) => (path.endsWith("stat") ? stat : bootId));

describe("Linux kernel birth evidence", () => {
  it("retains exact ticks and reads comm through its final parenthesis", () => {
    expect(parseLinuxStat(stat, 42)).toMatchObject({
      pid: 42,
      pgid: 42,
      startTicks: ticks,
      comm: "odd ) (name\n)",
    });
    expect(
      sameBirth(record, { ...record, args: "changed", comm: "changed", lstart: "changed" }),
    ).toBe(true);
  });
  it.each([
    { bootId: "different" },
    { startTicks: "100719925474099301" },
    { birthUnverifiable: true },
    { pgid: 51 },
  ])("refuses signals on uncertain ownership %o", async (delta) => {
    const actual = { ...record, ...delta };
    const signals = [];
    await expect(
      stopOwned(
        { native: record, exit: null },
        { birthImpl: () => actual, killImpl: (...args) => signals.push(args) },
      ),
    ).rejects.toMatchObject({
      code: "PID_REUSE",
      expected: record,
      actual,
      remaining: [actual],
      steps: [],
    });
    expect(signals).toEqual([]);
  });
  it("distinguishes ENOENT from an unreadable or malformed observation", () => {
    const fail = (code) => () => {
      throw Object.assign(new Error(code), { code });
    };
    expect(linuxBirth(42, fail("ENOENT"))).toBeNull();
    expect(linuxBirth(42, fail("EACCES"))).toMatchObject({ birthUnverifiable: true });
    expect(linuxBirth(42, () => "invalid")).toMatchObject({ birthUnverifiable: true });
    const legacy = { pid: 42, pgid: 42, lstart: "old" };
    expect(sameBirth(legacy, record)).toBe(false);
    expect(liveOwnedFromRegistry([legacy], () => record, "linux")).toEqual([legacy]);
  });
});

describe("Linux survivor verification", () => {
  const child = { ...record, pid: 43, ppid: 42, startTicks: "100719925474099302" };
  const row = (p) => `${p.pid} ${p.ppid} ${p.pgid} Sat Nov 21 03:10:30 2026 node`;
  function fixture(exit = null) {
    const live = new Map([
      [42, record],
      [43, child],
    ]);
    const handle = { native: record, known: new Map(live), exit };
    return {
      handle,
      live,
      options: {
        birthImpl: (pid) => live.get(pid) ?? null,
        psImpl: () => [...live.values()].map(row).join("\n"),
        intWaitMs: 0,
        termWaitMs: 0,
        killWaitMs: 0,
      },
    };
  }
  it("holds on an unreadable unknown group member after root exit", async () => {
    const { handle, live, options } = fixture({ exitCode: 0 });
    live.delete(42);
    handle.known.delete(43);
    const actual = { pid: 43, birthUnverifiable: true, error: "EACCES" };
    const signals = [];
    await expect(
      stopOwned(handle, {
        ...options,
        birthImpl: (pid) => (pid === 43 ? actual : null),
        killImpl: (...args) => signals.push(args),
      }),
    ).rejects.toMatchObject({
      actual,
      remaining: [actual],
      steps: [],
      code: "PID_REUSE",
    });
    expect(signals).toEqual([]);
  });
  it("honours cancellation after SIGINT without escalating", async () => {
    const { handle, options } = fixture();
    const controller = new AbortController();
    const signals = [];
    await expect(
      stopOwned(handle, {
        ...options,
        signal: controller.signal,
        killImpl(pid, signal) {
          signals.push([pid, signal]);
          controller.abort(new Error("terminal deadline"));
        },
      }),
    ).rejects.toThrow("terminal deadline");
    expect(signals).toEqual([[-42, "SIGINT"]]);
  });
  it("signals a previously verified surviving descendant individually after leader exit", async () => {
    const { handle, live, options } = fixture({ exitCode: 0 });
    live.delete(42);
    const signals = [];
    const report = await stopOwned(handle, {
      ...options,
      killImpl(pid, signal) {
        signals.push([pid, signal]);
        live.delete(pid);
      },
    });
    expect(signals).toEqual([[43, "SIGINT"]]);
    expect(report.absent).toBe(true);
  });
  it("refuses a reused group after SIGINT before escalation and preserves attempted signals", async () => {
    const { handle, live, options } = fixture();
    const signals = [];
    await expect(
      stopOwned(handle, {
        ...options,
        killImpl(pid, signal) {
          signals.push([pid, signal]);
          live.set(42, { ...record, startTicks: "777" });
        },
      }),
    ).rejects.toMatchObject({
      code: "PID_REUSE",
      differingFields: ["startTicks"],
      steps: [{ signal: "SIGINT" }],
      actual: { startTicks: "777" },
    });
    expect(signals).toEqual([[-42, "SIGINT"]]);
  });
});

describe("platform thread diagnostics", () => {
  it.each(["0", "", "-1", "x", "9007199254740993"])(
    "rejects malformed num_threads %j on both paths",
    async (value) => {
      const fields = ["S", "1", "42", ...Array(14).fill("0"), value, "0", ticks];
      const read = (path) => (path.endsWith("stat") ? `42 (weird )) ${fields.join(" ")}` : bootId);
      const options = { os: "linux", read };
      expect(threadCount(record, undefined, options)).toBe("unavailable");
      expect(await threadCountAsync(record, undefined, options)).toBe("unavailable");
    },
  );
  it.each(["ENOENT", "EACCES"])("reports unavailable for %s", async (code) => {
    const options = {
      os: "linux",
      read: () => {
        throw Object.assign(new Error(code), { code });
      },
    };
    expect(threadCount(record, undefined, options)).toBe("unavailable");
    expect(await threadCountAsync(record, undefined, options)).toBe("unavailable");
  });
  it("keeps the macOS thread-row command on both paths", async () => {
    const run = (cmd, args) => {
      expect([cmd, ...args]).toEqual(["ps", "-M", "-p", "42"]);
      return "USER PID\nu 42\nu 42\n";
    };
    expect(threadCount(42, run, { os: "darwin" })).toBe(2);
    expect(
      await threadCountAsync(42, (...args) => ({ stdout: run(...args) }), { os: "darwin" }),
    ).toBe(2);
  });
});
