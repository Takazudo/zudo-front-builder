#!/usr/bin/env node

// Verify the immutable registry tarball before describing local digests as
// shipped. --upload is the only path to the GitHub Release asset.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const NAME = "@takazudo/zfb-md-wasm";
const PATHS = [
  [".", "dist/wasm/zfb_md_wasm_bg.wasm"],
  ["./highlight", "dist/wasm-highlight/zfb_md_wasm_highlight_bg.wasm"],
  ["./render", "dist/wasm-render/zfb_md_wasm_render_bg.wasm"],
  ["./parse", "dist/wasm-parse/zfb_md_wasm_parse_bg.wasm"],
];

function fail(message) {
  throw new Error(`published @takazudo/zfb-md-wasm digest verification failed: ${message}`);
}

// npm registry read-after-write is not immediate: a just-published version can
// answer ETARGET/E404 for minutes (#4122). Only "not visible yet" is retried.
const NOT_VISIBLE = /\b(ETARGET|E404)\b|No matching version found|404 Not Found/;

function sleepMs(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function packWhenVisible(spec, destination) {
  const attempts = Number(process.env.ZFB_MD_WASM_VISIBILITY_ATTEMPTS ?? 40);
  const delayMs = Number(process.env.ZFB_MD_WASM_VISIBILITY_DELAY_MS ?? 15000);
  for (let attempt = 1; ; attempt++) {
    try {
      return execFileSync(
        "npm",
        ["pack", spec, "--pack-destination", destination, "--ignore-scripts"],
        { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
      );
    } catch (error) {
      const stderr = String(error.stderr ?? "");
      if (!NOT_VISIBLE.test(stderr)) {
        process.stderr.write(stderr);
        throw error;
      }
      if (attempt >= attempts) {
        fail(
          `${spec} never became visible on the registry after ${attempts} attempts ` +
            `(${(attempts * delayMs) / 1000}s); this is registry lag, not a digest mismatch`,
        );
      }
      console.log(
        `${spec} not visible on the registry yet (attempt ${attempt}/${attempts}); retrying in ${delayMs / 1000}s`,
      );
      sleepMs(delayMs);
    }
  }
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function main() {
  const args = process.argv.slice(2);
  const upload = args.includes("--upload");
  const allowMissingManifest = args.includes("--allow-missing-manifest");
  const value = (flag) => {
    const index = args.indexOf(flag);
    if (index < 0 || !args[index + 1]) fail(`missing ${flag}`);
    return args[index + 1];
  };
  const version = value("--version");
  const manifestPath = resolve(value("--manifest"));
  if (
    !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:next|beta|rc)\.(0|[1-9]\d*))?$/.test(version)
  ) {
    fail(`invalid version ${version}`);
  }
  const tag = upload ? value("--tag") : undefined;
  if (upload && tag !== `v${version}`) fail(`release tag ${tag} does not match v${version}`);
  if (!existsSync(manifestPath)) {
    if (!allowMissingManifest) fail(`local manifest is missing: ${manifestPath}`);
    console.log(
      `::warning::No shipped-artifacts.json in target tree for ${NAME}@${version}; older-tag recovery skips published digest verification and Release asset upload. Digests are NOT verified.`,
    );
    return;
  }

  const expected = readJson(manifestPath);
  if (
    expected.schemaVersion !== 1 ||
    expected.name !== NAME ||
    expected.version !== version ||
    !Array.isArray(expected.artifacts) ||
    expected.artifacts.length !== PATHS.length
  ) {
    fail("local manifest has wrong schema, package, version, or artifact count");
  }

  const temporary = mkdtempSync(join(tmpdir(), "zfb-md-wasm-published-"));
  try {
    const output = packWhenVisible(`${NAME}@${version}`, temporary);
    const filename = output.trim().split(/\r?\n/).at(-1);
    if (!filename || filename.includes("/") || filename.includes("\\"))
      fail("npm pack returned an invalid filename");
    const tarball = join(temporary, filename);
    execFileSync("tar", ["-xzf", tarball, "-C", temporary]);
    const packedRoot = join(temporary, "package");
    const packedPackage = readJson(join(packedRoot, "package.json"));
    if (packedPackage.name !== NAME || packedPackage.version !== version)
      fail("published package name/version differs");
    const packedManifestBytes = readFileSync(join(packedRoot, "dist/shipped-artifacts.json"));
    if (!packedManifestBytes.equals(readFileSync(manifestPath)))
      fail("published manifest differs from the local manifest");

    for (const [index, [entry, path]] of PATHS.entries()) {
      const artifact = expected.artifacts[index];
      if (
        artifact?.entry !== entry ||
        artifact.path !== path ||
        !Number.isSafeInteger(artifact.bytes) ||
        artifact.bytes < 0 ||
        typeof artifact.sha256 !== "string" ||
        !/^[0-9a-f]{64}$/.test(artifact.sha256)
      ) {
        fail(`invalid local manifest artifact ${index}`);
      }
      const bytes = readFileSync(join(packedRoot, path));
      const sha256 = createHash("sha256").update(bytes).digest("hex");
      if (bytes.length !== artifact.bytes || sha256 !== artifact.sha256) {
        fail(`${entry} (${path}) published bytes differ from the manifest`);
      }
      console.log(`${entry}: ${sha256} (${bytes.length} bytes)`);
    }
    console.log(`Verified all four published ${NAME}@${version} WASM digests.`);
    if (upload) {
      const asset = join(temporary, `zfb-md-wasm-${version}-shipped-artifacts.json`);
      copyFileSync(manifestPath, asset);
      execFileSync("gh", ["release", "upload", tag, asset, "--clobber"], { stdio: "inherit" });
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

try {
  main();
} catch (error) {
  console.error(`::error::${error.message}`);
  process.exitCode = 1;
}
