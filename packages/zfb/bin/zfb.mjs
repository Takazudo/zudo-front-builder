#!/usr/bin/env node
// Followed biome's pattern: pure os/cpu lookup → resolve platform package → spawn binary.
// See: https://github.com/biomejs/biome (packages/js/biome/bin/biome.mjs reference)
// Divergence from biome: async spawn + signal forwarding instead of spawnSync.
// biome is a short-lived CLI, but `zfb dev` is a long-running server — without
// forwarding, SIGTERM to the wrapper orphans the native binary (PPID 1) and
// leaves the dev-server port bound (issue #873).
import { existsSync } from "node:fs";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { constants as osConstants } from "node:os";
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

const child = spawn(binPath, process.argv.slice(2), { stdio: "inherit" });

// Forward termination signals to the child. Supervisors (concurrently
// --kill-others, Playwright webServer teardown, timeout(1), CI runners)
// signal only the wrapper; without forwarding, the native server survives
// with PPID 1 and keeps its port bound. Terminal Ctrl+C already signals the
// whole foreground group, so the child may receive SIGINT twice — the native
// binary treats repeated signals as the same shutdown request.
// SIGHUP is POSIX-only: on Windows child.kill("SIGHUP") throws (libuv only
// emulates SIGINT/SIGTERM/SIGKILL/SIGQUIT), and the OS tears down console
// processes on window close anyway.
const forwardedSignals =
  process.platform === "win32" ? ["SIGINT", "SIGTERM"] : ["SIGINT", "SIGTERM", "SIGHUP"];
const signalForwarders = new Map();
for (const sig of forwardedSignals) {
  const forward = () => child.kill(sig);
  signalForwarders.set(sig, forward);
  process.on(sig, forward);
}

// Surface spawn failures clearly. Without this, a 0644 binary (EACCES) is
// silently swallowed and the process exits with code 1 and no message —
// making it impossible for the user to diagnose a corrupted/incomplete npm
// install. (Issue #447 / #441 — pnpm publish strips the executable bit.)
child.on("error", (error) => {
  if (error.code === "EACCES") {
    process.stderr.write(
      `[zfb] binary is not executable; was the install corrupt?\n` +
        `      ${binPath}\n` +
        `      Try reinstalling: npm install --include=optional\n`,
    );
  } else {
    process.stderr.write(`[zfb] failed to spawn binary: ${error.message}\n` + `      ${binPath}\n`);
  }
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Re-raise the child's termination signal on ourselves so the caller
    // sees the real cause of death (e.g. WIFSIGNALED), not a plain exit code.
    // Remove our forwarding listener first or it would swallow the re-raise.
    const forward = signalForwarders.get(signal);
    if (forward) {
      process.off(signal, forward);
    }
    try {
      process.kill(process.pid, signal);
    } catch {
      // Signal cannot be re-raised on this platform (Windows emulation).
    }
    // Reached only if the re-raised signal did not terminate us — fall back
    // to the shell's 128+n convention for death-by-signal.
    const signum = osConstants.signals[signal];
    process.exit(signum ? 128 + signum : 1);
  }
  process.exit(code ?? 1);
});
