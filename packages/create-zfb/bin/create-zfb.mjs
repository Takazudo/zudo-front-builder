#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { constants as osConstants } from "node:os";
import { dirname, join } from "node:path";
import { checkForNewerVersion } from "./version-check.mjs";

const require = createRequire(import.meta.url);

try {
  const ownPkg = require("../package.json");
  const staleWarning = await checkForNewerVersion({ version: ownPkg.version });
  if (staleWarning) {
    process.stderr.write(staleWarning);
  }
} catch {
  // silently skip on any error: network failure, timeout (AbortError), parse error, etc.
}

const zfbPkgJson = require.resolve("@takazudo/zfb/package.json");
const zfbBin = join(dirname(zfbPkgJson), "bin", "zfb.mjs");

const args = ["new", ...process.argv.slice(2)];
const result = spawnSync(process.execPath, [zfbBin, ...args], { stdio: "inherit" });
if (result.signal) {
  // Re-raise the child's termination signal on ourselves so the caller sees
  // the real cause of death (e.g. WIFSIGNALED), not a plain exit code 1.
  // Mirrors the wrapper behaviour in packages/zfb/bin/zfb.mjs.
  try {
    process.kill(process.pid, result.signal);
  } catch {
    // Signal cannot be re-raised on this platform (Windows emulation).
  }
  // Reached only if the re-raised signal did not terminate us — fall back
  // to the shell's 128+n convention for death-by-signal.
  const signum = osConstants.signals[result.signal];
  process.exit(signum ? 128 + signum : 1);
}
process.exit(result.status ?? 1);
