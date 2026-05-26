#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);

// Returns -1 if a < b, 0 if a === b, 1 if a > b.
// Handles prerelease suffixes: 0.1.0-next.6 < 0.1.0-next.8 < 0.1.0
function compareSemver(a, b) {
  const splitRelease = (v) => {
    const [main, pre] = v.split(/-(.+)/, 2);
    return { parts: main.split(".").map(Number), pre: pre ?? null };
  };
  const ra = splitRelease(a);
  const rb = splitRelease(b);
  for (let i = 0; i < Math.max(ra.parts.length, rb.parts.length); i++) {
    const pa = ra.parts[i] ?? 0;
    const pb = rb.parts[i] ?? 0;
    if (pa !== pb) return pa < pb ? -1 : 1;
  }
  // same numeric parts — prerelease is less than no prerelease (semver rule)
  if (ra.pre !== null && rb.pre === null) return -1;
  if (ra.pre === null && rb.pre !== null) return 1;
  if (ra.pre !== null && rb.pre !== null) {
    if (ra.pre < rb.pre) return -1;
    if (ra.pre > rb.pre) return 1;
  }
  return 0;
}

try {
  const ownPkg = require("../package.json");
  const version = ownPkg.version;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 1500);
  const res = await fetch("https://registry.npmjs.org/create-zfb/latest", {
    signal: controller.signal,
    headers: {
      "User-Agent": `create-zfb/${version}`,
      Accept: "application/json",
    },
  });
  clearTimeout(timer);
  if (res.ok) {
    const data = await res.json();
    const latest = data?.version;
    if (typeof latest === "string" && typeof version === "string") {
      if (compareSemver(version, latest) < 0) {
        process.stderr.write(
          `[create-zfb] You are running create-zfb@${version} but @latest is ${latest}.\n` +
            `[create-zfb] pnpm 11's minimumReleaseAge may have downgraded the install.\n` +
            `[create-zfb] Re-run with: pnpm create zfb@latest --config.minimumReleaseAge=0\n` +
            `[create-zfb] Or use: npm create zfb@latest\n`,
        );
      }
    }
  }
} catch {
  // silently skip on any error: network failure, timeout (AbortError), parse error, etc.
}

const zfbPkgJson = require.resolve("@takazudo/zfb/package.json");
const zfbBin = join(dirname(zfbPkgJson), "bin", "zfb.mjs");

const args = ["new", ...process.argv.slice(2)];
const result = spawnSync(process.execPath, [zfbBin, ...args], { stdio: "inherit" });
process.exit(result.status ?? 1);
