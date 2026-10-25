// Installed-carrier helpers for the published-preset identity fixture (#4531).
// Importable on its own: run.mjs launches the whole fixture at import time.
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, realpath } from "node:fs/promises";
import { createRequire } from "node:module";
import path, { dirname, isAbsolute, join, relative, resolve } from "node:path";

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
  const packageDir = dirname(manifestPath);
  // No NODE_PATH / ancestor fallback: the carrier must live in this install root.
  const rel = relative(resolve(await realpath(installRoot), "node_modules"), packageDir);
  if (rel.startsWith("..") || isAbsolute(rel)) {
    throw new Error(`carrier ${packageName} resolved outside ${installRoot}: ${packageDir}`);
  }
  const binaryPath = join(packageDir, carrierBinaryName(platform));
  if (!existsSync(binaryPath)) throw new Error(`carrier binary does not exist: ${binaryPath}`);
  const carrierVersion = JSON.parse(await readFile(manifestPath, "utf8")).version;
  if (!carrierVersion) throw new Error(`carrier ${packageName} manifest has no version`);
  const sha256 = createHash("sha256")
    .update(await readFile(binaryPath))
    .digest("hex");
  return { packageName, packageDir, path: binaryPath, carrierVersion, sha256 };
}

// Resolve + pin assertion + lock integrity; throws rather than returning partial identity.
export async function installedCarrierIdentity(installRoot, siteRoot, options) {
  const carrier = await resolveInstalledCarrier(installRoot, options);
  assertCarrierVersion(carrier.carrierVersion, await readPinnedVersion(siteRoot));
  const lock = await readCarrierLockIntegrity(siteRoot, carrier.packageName);
  return { ...carrier, lockIntegrity: lock.integrity, lockVersion: lock.version };
}

const pathFor = (platform) => (platform === "win32" ? path.win32 : path.posix);

// Shell-free npm (#4532): `npm` is a .cmd shim on win32, so run npm-cli.js under the running Node.
export function locateNpmCli({
  platform = process.platform,
  execPath = process.execPath,
  env = process.env,
  exists = existsSync,
} = {}) {
  const p = pathFor(platform);
  const fromEnv = env.npm_execpath;
  if (fromEnv && /[\\/]npm-cli\.js$/.test(fromEnv) && exists(fromEnv)) return fromEnv;
  const nodeDir = p.dirname(execPath);
  const candidate =
    platform === "win32"
      ? p.join(nodeDir, "node_modules", "npm", "bin", "npm-cli.js")
      : p.join(p.dirname(nodeDir), "lib", "node_modules", "npm", "bin", "npm-cli.js");
  if (!exists(candidate)) {
    throw new Error(
      `npm-cli.js not found (npm_execpath=${fromEnv ?? "unset"}; looked for ${candidate}); ` +
        "run the fixture with a Node that ships npm",
    );
  }
  return candidate;
}

export function npmCommand(args, options = {}) {
  const execPath = options.execPath ?? process.execPath;
  return {
    command: execPath,
    args: [locateNpmCli({ ...options, execPath }), ...args],
  };
}

// Shell-free zudo-sg (#4532): node_modules/.bin/zudo-sg is a .cmd shim on win32, so run the
// installed package's own "bin" entry under the running Node.
export async function zudoSgCommand(
  installRoot,
  args,
  { platform = process.platform, execPath = process.execPath, readText = readFile } = {},
) {
  const p = pathFor(platform);
  const packageDir = p.join(installRoot, "node_modules", "@takazudo", "zudo-sg");
  const manifest = JSON.parse(await readText(p.join(packageDir, "package.json"), "utf8"));
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.["zudo-sg"];
  if (!bin) throw new Error(`@takazudo/zudo-sg at ${packageDir} declares no "zudo-sg" bin entry`);
  return { command: execPath, args: [p.resolve(packageDir, bin), ...args] };
}
