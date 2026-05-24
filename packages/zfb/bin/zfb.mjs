#!/usr/bin/env node
// Followed biome's pattern: pure os/cpu lookup → resolve platform package → spawn binary.
// See: https://github.com/biomejs/biome (packages/js/biome/bin/biome.mjs reference)
import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { join } from "node:path";

const require = createRequire(import.meta.url);

// Map of os-cpu keys to the corresponding optional platform package name.
// On Windows, npm generates zfb.cmd / zfb.ps1 wrappers from the bin field;
// the shebang above is used on Unix only.
const platformPackages = {
  "darwin-arm64": "@takazudo/zfb-darwin-arm64",
  "darwin-x64": "@takazudo/zfb-darwin-x64",
  "linux-arm64": "@takazudo/zfb-linux-arm64-gnu",
  "linux-x64": "@takazudo/zfb-linux-x64-gnu",
  "win32-x64": "@takazudo/zfb-win32-x64-msvc",
};

const key = `${process.platform}-${process.arch}`;
const pkg = platformPackages[key];

if (!pkg) {
  console.error(`[zfb] unsupported platform: ${key}`);
  process.exit(1);
}

let binPath;
try {
  // Resolve the platform package's package.json to get the install directory.
  const pkgJsonPath = require.resolve(`${pkg}/package.json`);
  const pkgDir = pkgJsonPath.replace(/[\\/]package\.json$/, "");
  const binName = process.platform === "win32" ? "zfb.exe" : "zfb";
  binPath = join(pkgDir, binName);
} catch {
  console.error(
    `[zfb] platform binary not installed: ${pkg}\n` +
      "      Some installers skip optionalDependencies. Reinstall with full deps,\n" +
      "      e.g. `npm install --include=optional` or `pnpm install` without\n" +
      "      `--no-optional` / `--ignore-scripts`.",
  );
  process.exit(1);
}

// Explicit existence check: the package directory may be linked (e.g. via pnpm
// workspace) even before the Wave-5 workflow places the real binary. Without
// this guard, spawnSync would silently ENOENT instead of printing a clear error.
if (!existsSync(binPath)) {
  console.error(
    `[zfb] platform binary not installed: ${pkg}\n` +
      "      Some installers skip optionalDependencies. Reinstall with full deps,\n" +
      "      e.g. `npm install --include=optional` or `pnpm install` without\n" +
      "      `--no-optional` / `--ignore-scripts`.",
  );
  process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });

// Surface spawn errors that spawnSync stores in result.error rather than
// propagating to stderr. Without this, a 0644 binary (EACCES) is silently
// swallowed and the process exits with code 1 and no message — making it
// impossible for the user to diagnose a corrupted/incomplete npm install.
// (Issue #447 / #441 — pnpm publish strips the executable bit.)
if (result.error) {
  if (result.error.code === "EACCES") {
    process.stderr.write(
      `[zfb] binary is not executable; was the install corrupt?\n` +
        `      ${binPath}\n` +
        `      Try reinstalling: npm install --include=optional\n`,
    );
  } else {
    process.stderr.write(
      `[zfb] failed to spawn binary: ${result.error.message}\n` + `      ${binPath}\n`,
    );
  }
  process.exit(1);
}

process.exit(result.status ?? 1);
