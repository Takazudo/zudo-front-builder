import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import {
  PLATFORM_PACKAGES,
  assertCarrierVersion,
  carrierPackageName,
  installedCarrierIdentity,
  parseBinaryMode,
  readCarrierLockIntegrity,
  resolveInstalledCarrier,
} from "../../tests/published-preset-identity/carrier.mjs";

const temps = [];
afterEach(() => {
  while (temps.length) rmSync(temps.pop(), { recursive: true, force: true });
});

const INTEGRITY = "sha512-abc";

function fakeTrees({
  carrierVersion = "4.7.0",
  pinned = "4.7.0",
  integrity = INTEGRITY,
  binary = true,
} = {}) {
  const base = mkdtempSync(join(tmpdir(), "carrier-test-"));
  temps.push(base);
  const installRoot = join(base, "install");
  const siteRoot = join(base, "site");
  mkdirSync(installRoot);
  mkdirSync(siteRoot);
  writeFileSync(join(installRoot, "package.json"), "{}");
  const pkg = PLATFORM_PACKAGES["linux-x64"];
  const dir = join(installRoot, "node_modules", pkg);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: pkg, version: carrierVersion }));
  if (binary) writeFileSync(join(dir, "zfb"), "binary-bytes");
  writeFileSync(
    join(siteRoot, "package.json"),
    JSON.stringify({ dependencies: { "@takazudo/zfb": pinned } }),
  );
  writeFileSync(
    join(siteRoot, "package-lock.json"),
    JSON.stringify({
      packages: { [`node_modules/${pkg}`]: { version: "4.7.0", ...(integrity && { integrity }) } },
    }),
  );
  return { installRoot, siteRoot, dir };
}

describe("published-preset carrier helper (#4531)", () => {
  it.each([
    ["darwin-arm64", "@takazudo/zfb-darwin-arm64"],
    ["darwin-x64", "@takazudo/zfb-darwin-x64"],
    ["linux-arm64", "@takazudo/zfb-linux-arm64-gnu"],
    ["linux-x64", "@takazudo/zfb-linux-x64-gnu"],
    ["win32-x64", "@takazudo/zfb-win32-x64-msvc"],
  ])("maps %s to %s", (key, name) => {
    const [platform, arch] = key.split("-");
    expect(carrierPackageName(platform, arch)).toBe(name);
  });

  it("throws on an unsupported platform", () => {
    expect(() => carrierPackageName("freebsd", "x64")).toThrow(/unsupported platform/);
  });

  it("parses the binary mode and rejects anything else", () => {
    expect(parseBinaryMode(undefined)).toBe("workspace");
    expect(parseBinaryMode("installed")).toBe("installed");
    expect(() => parseBinaryMode("Installed")).toThrow(/ZFB_PRESET_BINARY/);
  });

  it("throws on a carrier/pin version mismatch", () => {
    expect(() => assertCarrierVersion("4.7.1", "4.7.0")).toThrow(/does not equal pinned/);
    expect(() => assertCarrierVersion(undefined, "4.7.0")).toThrow();
    expect(() => assertCarrierVersion("4.7.0", "4.7.0")).not.toThrow();
  });

  it("resolves the carrier inside the given install root", async () => {
    const { installRoot, dir } = fakeTrees();
    const carrier = await resolveInstalledCarrier(installRoot, { platform: "linux", arch: "x64" });
    expect(carrier.path.endsWith(join(dir, "zfb").replace(/^.*node_modules/, "node_modules"))).toBe(
      true,
    );
    expect(carrier.carrierVersion).toBe("4.7.0");
    expect(carrier.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("throws when the carrier package or binary is missing", async () => {
    const empty = mkdtempSync(join(tmpdir(), "carrier-test-"));
    temps.push(empty);
    writeFileSync(join(empty, "package.json"), "{}");
    await expect(
      resolveInstalledCarrier(empty, { platform: "linux", arch: "x64" }),
    ).rejects.toThrow(/not installed|outside/);
    const { installRoot } = fakeTrees({ binary: false });
    await expect(
      resolveInstalledCarrier(installRoot, { platform: "linux", arch: "x64" }),
    ).rejects.toThrow(/does not exist/);
  });

  it("reads the lock integrity and fails closed when it is missing", async () => {
    const ok = fakeTrees();
    expect(
      (await readCarrierLockIntegrity(ok.siteRoot, PLATFORM_PACKAGES["linux-x64"])).integrity,
    ).toBe(INTEGRITY);
    const bad = fakeTrees({ integrity: null });
    await expect(
      readCarrierLockIntegrity(bad.siteRoot, PLATFORM_PACKAGES["linux-x64"]),
    ).rejects.toThrow(/no sha512 integrity/);
  });

  it("builds a full identity and rejects a version mismatch end to end", async () => {
    const opts = { platform: "linux", arch: "x64" };
    const ok = fakeTrees();
    const identity = await installedCarrierIdentity(ok.installRoot, ok.siteRoot, opts);
    expect(identity.lockIntegrity).toBe(INTEGRITY);
    const mismatch = fakeTrees({ carrierVersion: "4.6.0" });
    await expect(
      installedCarrierIdentity(mismatch.installRoot, mismatch.siteRoot, opts),
    ).rejects.toThrow(/does not equal pinned/);
  });
});
