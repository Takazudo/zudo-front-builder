// Unit tests for the musl-libc detection used by the zfb launcher
// (packages/zfb/bin/zfb.mjs) to give Alpine/musl-libc Linux users a friendly
// error instead of a raw dynamic-loader exec failure (the gnu-built binary
// matches on os/cpu but not on libc). detectMuslLinux is dependency-injected
// (readdirSyncFn / getReportFn / platform) so these tests never touch the
// real filesystem or process.report, and importing this module has no
// exec/spawn side effects (unlike importing bin/zfb.mjs directly).
import { describe, expect, it } from "vitest";

import { detectMuslLinux } from "../../bin/detect-musl.mjs";

function readdirWith(names: string[]) {
  return () => names;
}

function readdirThrows() {
  return () => {
    throw new Error("ENOENT: no such file or directory, scandir '/lib'");
  };
}

function reportWithGlibc(version = "2.31") {
  return () => ({ header: { glibcVersionRuntime: version } });
}

function reportWithoutGlibc() {
  return () => ({ header: {} });
}

function reportThrows() {
  return () => {
    throw new Error("report unavailable");
  };
}

describe("detectMuslLinux", () => {
  it("returns false on darwin regardless of fs/report state (zero behavior change)", () => {
    expect(
      detectMuslLinux({
        platform: "darwin",
        readdirSyncFn: readdirWith(["ld-musl-x86_64.so.1"]),
        getReportFn: reportWithoutGlibc(),
      }),
    ).toBe(false);
  });

  it("returns false on win32 regardless of fs/report state (zero behavior change)", () => {
    expect(
      detectMuslLinux({
        platform: "win32",
        readdirSyncFn: readdirWith(["ld-musl-x86_64.so.1"]),
        getReportFn: reportWithoutGlibc(),
      }),
    ).toBe(false);
  });

  it("returns false on ordinary glibc linux (no musl loader, glibcVersionRuntime present)", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirWith(["libc.so.6", "ld-linux-x86-64.so.2"]),
        getReportFn: reportWithGlibc(),
      }),
    ).toBe(false);
  });

  it("returns true on linux when a musl loader file is present (e.g. Alpine)", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirWith(["ld-musl-x86_64.so.1"]),
        // Even if the report check would say "glibc" (shouldn't happen in
        // practice), the loader-file probe is authoritative and wins.
        getReportFn: reportWithGlibc(),
      }),
    ).toBe(true);
  });

  it("detects the aarch64 musl loader filename too", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirWith(["ld-musl-aarch64.so.1"]),
        getReportFn: reportWithGlibc(),
      }),
    ).toBe(true);
  });

  it("falls back to the glibcVersionRuntime absence check when /lib can't be read", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirThrows(),
        getReportFn: reportWithoutGlibc(),
      }),
    ).toBe(true);
  });

  it("falls back to false when /lib can't be read but glibcVersionRuntime is present", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirThrows(),
        getReportFn: reportWithGlibc(),
      }),
    ).toBe(false);
  });

  it("treats a throwing process.report as inconclusive, not a crash (returns true, matching absence)", () => {
    expect(
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirWith([]),
        getReportFn: reportThrows(),
      }),
    ).toBe(true);
  });

  it("never throws even when both probes fail", () => {
    expect(() =>
      detectMuslLinux({
        platform: "linux",
        readdirSyncFn: readdirThrows(),
        getReportFn: reportThrows(),
      }),
    ).not.toThrow();
  });

  it("uses the real process.platform/fs/process.report when called with no arguments", () => {
    // Smoke test only: on this dev/CI host (darwin or a normal glibc linux
    // runner) it must resolve to false without throwing.
    expect(() => detectMuslLinux()).not.toThrow();
    if (process.platform !== "linux") {
      expect(detectMuslLinux()).toBe(false);
    }
  });
});
