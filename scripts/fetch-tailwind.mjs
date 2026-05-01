#!/usr/bin/env node
// Fetches the pinned tailwindcss v4 standalone CLI binary into
// crates/zfb/binaries/tailwindcss-v4 (or .exe on Windows). Idempotent:
// re-runs are a fast no-op when the on-disk binary already matches the
// pinned sha256. Honours ZFB_TAILWIND_BIN to skip entirely when the user
// has supplied their own binary.
//
// Run via `pnpm fetch:tailwind` from the workspace root.
//
// The version below MUST stay in sync with the pin documented in
// crates/zfb-css/README.md ("Tailwind CSS Version" section). When that
// pin is bumped, bump this constant in the same commit. Until we add a
// shared workspace pin file, this is the only sync point.
const TAILWIND_VERSION = "4.2.0";

import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const BINARY_DIR = join(REPO_ROOT, "crates", "zfb", "binaries");
const IS_WINDOWS = process.platform === "win32";
const BINARY_NAME = IS_WINDOWS ? "tailwindcss-v4.exe" : "tailwindcss-v4";
const BINARY_PATH = join(BINARY_DIR, BINARY_NAME);

const RELEASE_BASE = `https://github.com/tailwindlabs/tailwindcss/releases/download/v${TAILWIND_VERSION}`;

function detectAssetName() {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "darwin" && arch === "x64") return "tailwindcss-macos-x64";
  if (platform === "darwin" && arch === "arm64") return "tailwindcss-macos-arm64";
  if (platform === "linux" && arch === "x64") return "tailwindcss-linux-x64";
  if (platform === "linux" && arch === "arm64") return "tailwindcss-linux-arm64";
  if (platform === "win32" && arch === "x64") return "tailwindcss-windows-x64.exe";
  throw new Error(
    `Unsupported platform/arch: ${platform}/${arch}. Supported: ` +
      `darwin-x64, darwin-arm64, linux-x64, linux-arm64, win32-x64. ` +
      `For musl-libc Linux, set ZFB_TAILWIND_BIN to a manually-fetched binary.`,
  );
}

async function fetchToBuffer(url) {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(`GET ${url} failed: ${res.status} ${res.statusText}`);
  }
  return Buffer.from(await res.arrayBuffer());
}

function sha256(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

function parseSha256Sums(text, assetName) {
  // Format: "<hex>  <filename>" per line. Filenames may carry a leading
  // "./" (the tailwindlabs release uses this form) or a "*" binary-mode
  // marker (GNU coreutils sha256sum -c convention).
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = trimmed.match(/^([0-9a-f]{64})\s+\*?(\S+)$/i);
    if (!match) continue;
    const [, hex, rawName] = match;
    const name = rawName.replace(/^\.\//, "");
    if (name === assetName) return hex.toLowerCase();
  }
  throw new Error(`Asset ${assetName} not found in sha256sums.txt for v${TAILWIND_VERSION}.`);
}

function existingBinaryMatches(expectedSha) {
  if (!existsSync(BINARY_PATH)) return false;
  const buf = readFileSync(BINARY_PATH);
  return sha256(buf) === expectedSha;
}

async function main() {
  if (process.env.ZFB_TAILWIND_BIN) {
    const overridePath = process.env.ZFB_TAILWIND_BIN;
    if (!existsSync(overridePath)) {
      console.error(
        `ZFB_TAILWIND_BIN is set to ${overridePath} but the file does not exist. ` +
          `Either fix the path or unset the variable to fall back to the workspace fetch.`,
      );
      process.exit(1);
    }
    if (!statSync(overridePath).isFile()) {
      console.error(
        `ZFB_TAILWIND_BIN is set to ${overridePath} but it is not a regular file ` +
          `(directory, device, or broken symlink). Point it at the tailwindcss binary itself.`,
      );
      process.exit(1);
    }
    console.log(`ZFB_TAILWIND_BIN points at ${overridePath} — skipping workspace fetch.`);
    return;
  }

  const assetName = detectAssetName();
  console.log(
    `Resolving tailwindcss v${TAILWIND_VERSION} for ${process.platform}-${process.arch} (${assetName})...`,
  );

  const sumsUrl = `${RELEASE_BASE}/sha256sums.txt`;
  const sumsBuf = await fetchToBuffer(sumsUrl);
  const expectedSha = parseSha256Sums(sumsBuf.toString("utf8"), assetName);

  if (existingBinaryMatches(expectedSha)) {
    console.log(`✓ ${BINARY_PATH} already matches sha256 ${expectedSha} — nothing to do.`);
    return;
  }

  mkdirSync(BINARY_DIR, { recursive: true });

  const binaryUrl = `${RELEASE_BASE}/${assetName}`;
  console.log(`Downloading ${binaryUrl}...`);
  const binaryBuf = await fetchToBuffer(binaryUrl);

  const actualSha = sha256(binaryBuf);
  if (actualSha !== expectedSha) {
    throw new Error(
      `Checksum mismatch for ${assetName}: expected ${expectedSha}, got ${actualSha}. ` +
        `Refusing to install. The release may have been re-cut, or the download was corrupted.`,
    );
  }

  // Atomic-ish replace: write to .tmp then rename.
  const tmpPath = `${BINARY_PATH}.tmp`;
  writeFileSync(tmpPath, binaryBuf);
  if (!IS_WINDOWS) chmodSync(tmpPath, 0o755);
  renameSync(tmpPath, BINARY_PATH);

  console.log(`✓ Verified sha256 ${actualSha}`);
  console.log(`✓ Installed at ${BINARY_PATH}`);
}

main().catch((err) => {
  console.error(`fetch-tailwind: ${err.message}`);
  process.exit(1);
});
