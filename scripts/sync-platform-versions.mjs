#!/usr/bin/env node
/**
 * sync-platform-versions.mjs
 *
 * Keep packages/zfb-{platform}/package.json version fields,
 * packages/zfb/package.json optionalDependencies entries
 * (prefixed with @takazudo/zfb-), and related workspace package version
 * fields in lockstep with packages/zfb/package.json version.
 *
 * Usage:
 *   node scripts/sync-platform-versions.mjs
 *   pnpm sync:platform-versions
 *
 * Exits non-zero with a clear error if the source version is missing or invalid.
 * Re-running the script on an already-synced tree is a no-op.
 *
 * Uses only Node stdlib. Requires Node >= 18.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PLATFORMS = ["darwin-arm64", "darwin-x64", "linux-x64-gnu", "win32-x64-msvc"];

const OPTIONAL_DEP_PREFIX = "@takazudo/zfb-";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const repoRoot = resolve(__dirname, "..");

/**
 * Read a JSON file from disk. Returns parsed data and the original raw text
 * so callers can detect no-op writes.
 */
function readJson(path) {
  const raw = readFileSync(path, "utf8");
  return { raw, data: JSON.parse(raw) };
}

/**
 * Serialize JSON with 2-space indentation and a trailing newline.
 */
function stringifyJson(data) {
  return JSON.stringify(data, null, 2) + "\n";
}

/**
 * Write JSON file only if content changed (no-op on already-synced files).
 */
function writeJsonIfChanged(path, data, raw) {
  const nextRaw = stringifyJson(data);
  if (nextRaw !== raw) {
    writeFileSync(path, nextRaw);
    return true;
  }
  return false;
}

/**
 * Fail with a clear error message and non-zero exit.
 */
function fail(message) {
  process.stderr.write(`sync-platform-versions: ${message}\n`);
  process.exit(1);
}

function main() {
  // Source-of-truth: packages/zfb/package.json (NOT the workspace root).
  // The workspace root package.json stays private/0.0.0 and is not read here.
  const zfbPkgPath = resolve(repoRoot, "packages", "zfb", "package.json");
  const { raw: zfbRaw, data: zfbPkg } = readJson(zfbPkgPath);

  const srcVersion = zfbPkg.version;
  if (typeof srcVersion !== "string" || srcVersion.length === 0) {
    fail('packages/zfb/package.json "version" is missing or invalid');
  }

  process.stdout.write(`sync-platform-versions: source version = ${srcVersion}\n`);

  // 1. Rewrite packages/zfb-{platform}/package.json version fields.
  for (const platform of PLATFORMS) {
    const pkgPath = resolve(repoRoot, "packages", `zfb-${platform}`, "package.json");
    const { raw, data: pkg } = readJson(pkgPath);
    const previousVersion = pkg.version;
    pkg.version = srcVersion;
    const changed = writeJsonIfChanged(pkgPath, pkg, raw);
    if (changed) {
      process.stdout.write(
        `  packages/zfb-${platform}/package.json: ${previousVersion} -> ${srcVersion}\n`,
      );
    } else {
      process.stdout.write(
        `  packages/zfb-${platform}/package.json: already ${srcVersion} (no change)\n`,
      );
    }
  }

  // 2. Rewrite packages/zfb/package.json optionalDependencies entries matching the prefix.
  // (version field itself is the source — do NOT modify it.)
  const optionalDeps = zfbPkg.optionalDependencies;
  if (optionalDeps && typeof optionalDeps === "object") {
    process.stdout.write(`  packages/zfb/package.json optionalDependencies:\n`);
    for (const key of Object.keys(optionalDeps)) {
      if (!key.startsWith(OPTIONAL_DEP_PREFIX)) continue;
      const previousValue = optionalDeps[key];
      // Preserve "workspace:" prefix if present — pnpm 10 needs it for the
      // workspace link to resolve during dev. pnpm publish rewrites it to the
      // bare version at publish time.
      const next = String(previousValue).startsWith("workspace:")
        ? `workspace:${srcVersion}`
        : srcVersion;
      optionalDeps[key] = next;
      if (optionalDeps[key] !== previousValue) {
        process.stdout.write(`    ${key}: ${previousValue} -> ${next}\n`);
      } else {
        process.stdout.write(`    ${key}: already ${next} (no change)\n`);
      }
    }
  }
  writeJsonIfChanged(zfbPkgPath, zfbPkg, zfbRaw);

  // 3. Rewrite packages/zfb-runtime/package.json version field.
  // NOTE: peerDependencies."@takazudo/zfb": "workspace:*" is intentionally left alone.
  // pnpm publish rewrites workspace: protocol at publish time — do NOT mutate it here.
  {
    const pkgPath = resolve(repoRoot, "packages", "zfb-runtime", "package.json");
    const { raw, data: pkg } = readJson(pkgPath);
    const previousVersion = pkg.version;
    pkg.version = srcVersion;
    const changed = writeJsonIfChanged(pkgPath, pkg, raw);
    if (changed) {
      process.stdout.write(
        `  packages/zfb-runtime/package.json: ${previousVersion} -> ${srcVersion}\n`,
      );
    } else {
      process.stdout.write(
        `  packages/zfb-runtime/package.json: already ${srcVersion} (no change)\n`,
      );
    }
  }

  // 4. Rewrite packages/zfb-adapter-cloudflare/package.json version field.
  {
    const pkgPath = resolve(repoRoot, "packages", "zfb-adapter-cloudflare", "package.json");
    const { raw, data: pkg } = readJson(pkgPath);
    const previousVersion = pkg.version;
    pkg.version = srcVersion;
    const changed = writeJsonIfChanged(pkgPath, pkg, raw);
    if (changed) {
      process.stdout.write(
        `  packages/zfb-adapter-cloudflare/package.json: ${previousVersion} -> ${srcVersion}\n`,
      );
    } else {
      process.stdout.write(
        `  packages/zfb-adapter-cloudflare/package.json: already ${srcVersion} (no change)\n`,
      );
    }
  }

  // 5. Rewrite packages/create-zfb/package.json:
  //    - version field
  //    - dependencies."@takazudo/zfb" (preserving "workspace:" prefix if present)
  {
    const pkgPath = resolve(repoRoot, "packages", "create-zfb", "package.json");
    const { raw, data: pkg } = readJson(pkgPath);
    const previousVersion = pkg.version;
    pkg.version = srcVersion;

    if (pkg.dependencies && typeof pkg.dependencies["@takazudo/zfb"] === "string") {
      const curr = pkg.dependencies["@takazudo/zfb"];
      // If value starts with "workspace:", preserve that prefix and update only the version suffix.
      // workspace: protocol is required by pnpm 10 for intra-workspace deps.
      const next = curr.startsWith("workspace:") ? `workspace:${srcVersion}` : srcVersion;
      const changed = curr !== next;
      pkg.dependencies["@takazudo/zfb"] = next;
      if (changed) {
        process.stdout.write(
          `  packages/create-zfb/package.json dependencies["@takazudo/zfb"]: ${curr} -> ${next}\n`,
        );
      } else {
        process.stdout.write(
          `  packages/create-zfb/package.json dependencies["@takazudo/zfb"]: already ${next} (no change)\n`,
        );
      }
    }

    const changed = writeJsonIfChanged(pkgPath, pkg, raw);
    if (changed) {
      process.stdout.write(
        `  packages/create-zfb/package.json version: ${previousVersion} -> ${srcVersion}\n`,
      );
    } else {
      process.stdout.write(
        `  packages/create-zfb/package.json: already ${srcVersion} (no change)\n`,
      );
    }
  }
}

main();
