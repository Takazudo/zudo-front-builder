// Installed-carrier helpers for the published-preset identity fixture (#4531).
// Importable on its own: run.mjs launches the whole fixture at import time.
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, realpath } from "node:fs/promises";
import { createRequire } from "node:module";
import { isAbsolute, join, relative, resolve } from "node:path";

// Mirrors packages/zfb/bin/zfb.mjs:20-56 (os-cpu key -> native carrier package).
export const PLATFORM_PACKAGES = {
  "darwin-arm64": "@takazudo/zfb-darwin-arm64",
  "darwin-x64": "@takazudo/zfb-darwin-x64",
  "linux-arm64": "@takazudo/zfb-linux-arm64-gnu",
  "linux-x64": "@takazudo/zfb-linux-x64-gnu",
  "win32-x64": "@takazudo/zfb-win32-x64-msvc",
};

export const BINARY_MODES = ["workspace", "installed"];

export function parseBinaryMode(value) {
  const mode = value === undefined || value === "" ? "workspace" : value;
  if (!BINARY_MODES.includes(mode)) {
    throw new Error(
      `ZFB_PRESET_BINARY must be one of ${BINARY_MODES.join(" | ")}, got ${JSON.stringify(value)}`,
    );
  }
  return mode;
}

export function carrierPackageName(platform = process.platform, arch = process.arch) {
  const name = PLATFORM_PACKAGES[`${platform}-${arch}`];
  if (!name) throw new Error(`unsupported platform for the zfb carrier: ${platform}-${arch}`);
  return name;
}

export function carrierBinaryName(platform = process.platform) {
  return platform === "win32" ? "zfb.exe" : "zfb";
}

export async function readPinnedVersion(siteRoot, name = "@takazudo/zfb") {
  const manifest = JSON.parse(await readFile(join(siteRoot, "package.json"), "utf8"));
  const pinned = manifest.dependencies?.[name];
  if (!pinned) throw new Error(`${name} is not pinned in ${join(siteRoot, "package.json")}`);
  return pinned;
}

export async function readCarrierLockIntegrity(siteRoot, packageName) {
  const lock = JSON.parse(await readFile(join(siteRoot, "package-lock.json"), "utf8"));
  const entry = lock.packages?.[`node_modules/${packageName}`];
  if (!entry?.integrity || !/^sha512-/.test(entry.integrity)) {
    throw new Error(`package-lock.json has no sha512 integrity for ${packageName}`);
  }
  return { version: entry.version, integrity: entry.integrity, resolved: entry.resolved };
}

export function assertCarrierVersion(carrierVersion, pinnedVersion) {
  if (!carrierVersion || carrierVersion !== pinnedVersion) {
    throw new Error(
      `installed carrier version ${carrierVersion} does not equal pinned @takazudo/zfb ${pinnedVersion}`,
    );
  }
}

// Resolves the carrier binary installed inside `installRoot` only (no fallback).
export async function resolveInstalledCarrier(
  installRoot,
  { platform = process.platform, arch = process.arch } = {},
) {
  const packageName = carrierPackageName(platform, arch);
  let manifestPath;
  try {
    manifestPath = createRequire(join(installRoot, "package.json")).resolve(
      `${packageName}/package.json`,
    );
  } catch (error) {
    throw new Error(
      `carrier ${packageName} is not installed under ${installRoot}: ${error.message}`,
    );
  }
  const packageDir = manifestPath.replace(/[\\/]package\.json$/, "");
  // No NODE_PATH / ancestor fallback: the carrier must live in this install root.
  const rel = relative(resolve(await realpath(installRoot), "node_modules"), packageDir);
  if (rel.startsWith("..") || isAbsolute(rel)) {
    throw new Error(`carrier ${packageName} resolved outside ${installRoot}: ${packageDir}`);
  }
  const path = join(packageDir, carrierBinaryName(platform));
  if (!path || !existsSync(path)) throw new Error(`carrier binary does not exist: ${path}`);
  const carrierVersion = JSON.parse(await readFile(manifestPath, "utf8")).version;
  if (!carrierVersion) throw new Error(`carrier ${packageName} manifest has no version`);
  const sha256 = createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
  return { packageName, packageDir, path, carrierVersion, sha256 };
}

// Resolve + pin assertion + lock integrity; throws rather than returning partial identity.
export async function installedCarrierIdentity(installRoot, siteRoot, options) {
  const carrier = await resolveInstalledCarrier(installRoot, options);
  assertCarrierVersion(carrier.carrierVersion, await readPinnedVersion(siteRoot));
  const lock = await readCarrierLockIntegrity(siteRoot, carrier.packageName);
  return { ...carrier, lockIntegrity: lock.integrity, lockVersion: lock.version };
}
