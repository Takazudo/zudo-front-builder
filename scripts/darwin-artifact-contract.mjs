#!/usr/bin/env node
// Shared contract for owner-Mac Darwin release inputs (#4689). The producer
// (scripts/build-darwin-local.sh) writes receipts through this module and the
// Linux-runnable admission verifier (scripts/admit-darwin-artifacts.mjs) reads
// the same constants, so the two cannot drift. Human-readable contract:
// scripts/darwin-artifact-contract.md.
import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";

export const RECEIPT_SCHEMA = "zfb-darwin-qualification/v1";
export const VERDICT_SCHEMA = "zfb-darwin-admission/v1";

export const X64_LIMITATION =
  "functional only under Rosetta 2; no native Intel performance/timing/CPU-feature claim";

// Mach-O CPU types (mach/machine.h): CPU_TYPE_ARM64 = 0x0100000c, CPU_TYPE_X86_64 = 0x01000007.
export const DARWIN_TARGETS = Object.freeze({
  "aarch64-apple-darwin": Object.freeze({
    archLabel: "native-arm64",
    translated: 0,
    unameM: "arm64",
    cpuType: 0x0100000c,
    requiredChecks: Object.freeze([
      "macho-binary-arch",
      "macho-addon-arch",
      "proc-translated",
      "cli-version",
      "cli-help",
      "node-identity",
      "css-addon-smoke",
    ]),
  }),
  "x86_64-apple-darwin": Object.freeze({
    archLabel: "rosetta-translated",
    translated: 1,
    unameM: "x86_64",
    cpuType: 0x01000007,
    requiredChecks: Object.freeze([
      "macho-binary-arch",
      "macho-addon-arch",
      "proc-translated",
      "cli-version",
      "cli-help",
      "node-identity",
      "css-addon-smoke",
    ]),
  }),
});

// Inputs whose bytes decide the shipped binary: Cargo.lock pins every Rust
// crate, rust-toolchain.toml selects the compiler channel, and pnpm-lock.yaml
// pins the node_modules tree that crates/zfb's build.rs embeds at compile time.
export const INPUT_FILES = Object.freeze(["Cargo.lock", "rust-toolchain.toml", "pnpm-lock.yaml"]);

export function assetNames(version, target) {
  const archive = `zfb-${version}-${target}.tar.gz`;
  const addon = `zfb-css-${version}-${target}.node`;
  return {
    archive,
    archiveSha256: `${archive}.sha256`,
    addon,
    addonSha256: `${addon}.sha256`,
    receipt: `zfb-${version}-${target}.qualification.json`,
  };
}

export const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
export const OBJECT_ID_RE = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
export const SHA256_RE = /^[0-9a-f]{64}$/;

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function describeBytes(name, bytes) {
  return { name, sha256: sha256(bytes), size: bytes.length };
}

function parseFlags(argv) {
  const flags = {};
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--") {
      rest.push(...argv.slice(i + 1));
      break;
    }
    if (!arg.startsWith("--")) throw new Error(`unexpected argument: ${arg}`);
    const value = argv[i + 1];
    if (value === undefined) throw new Error(`${arg} needs a value`);
    flags[arg.slice(2)] = value;
    i++;
  }
  return { flags, rest };
}

function need(flags, key) {
  if (flags[key] === undefined) throw new Error(`missing --${key}`);
  return flags[key];
}

// record-check: append one qualification check (JSON line). The check passes
// only when the command exited 0 and, with --expect, its trimmed stdout equals
// the expected value. Checks run once; there is no retry path.
export function recordCheck({ file, name, exitCode, output, expect, command }) {
  const trimmed = output.trim();
  const pass = exitCode === 0 && (expect === undefined || trimmed === expect);
  const entry = {
    name,
    command,
    exit_code: exitCode,
    first_result: pass ? "pass" : "fail",
    retries: 0,
    output: trimmed.length > 2000 ? `${trimmed.slice(0, 2000)}…` : trimmed,
  };
  if (expect !== undefined) entry.expected = expect;
  appendFileSync(file, `${JSON.stringify(entry)}\n`);
  return entry;
}

export function buildReceipt(opts) {
  const spec = DARWIN_TARGETS[opts.target];
  if (!spec) throw new Error(`unsupported target: ${opts.target}`);
  const names = assetNames(opts.version, opts.target);
  const read = (path) => readFileSync(path);
  const inputs = {};
  for (const file of INPUT_FILES) inputs[file] = sha256(read(join(opts.repoRoot, file)));
  const checks = readFileSync(opts.checksFile, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const checkNames = new Set(checks.map((check) => check.name));
  const allRequired = spec.requiredChecks.every((name) => checkNames.has(name));
  const passed = allRequired && checks.every((c) => c.first_result === "pass" && c.exit_code === 0);
  return {
    schema: RECEIPT_SCHEMA,
    version: opts.version,
    tag: opts.tag || null,
    target: opts.target,
    source: { commit: opts.commit, tree: opts.tree },
    host: {
      uname_m: opts.unameM,
      os_version: opts.osVersion,
      hw_optional_arm64: opts.hwArm64,
      translated: Number(opts.translated),
      arch_label: spec.archLabel,
    },
    toolchain: {
      rustc_vV: opts.rustcVV,
      cargo_V: opts.cargoV,
      rust_toolchain_toml_sha256: inputs["rust-toolchain.toml"],
      cargo_lock_sha256: inputs["Cargo.lock"],
      profile: "release",
      features: "default",
      env: {
        ZFB_RELEASE_VERSION: opts.version,
        CARGO_BUILD_JOBS: opts.cargoBuildJobs || null,
      },
    },
    inputs,
    artifacts: {
      binary: describeBytes("zfb", read(opts.binary)),
      addon: describeBytes(names.addon, read(join(opts.outDir, names.addon))),
      archive: describeBytes(names.archive, read(join(opts.outDir, names.archive))),
      archive_sha256_file: describeBytes(
        names.archiveSha256,
        read(join(opts.outDir, names.archiveSha256)),
      ),
      addon_sha256_file: describeBytes(
        names.addonSha256,
        read(join(opts.outDir, names.addonSha256)),
      ),
    },
    file_output: { binary: opts.fileBinary, addon: opts.fileAddon },
    qualification: { checks, status: passed ? "passed" : "failed" },
    created_at: new Date().toISOString(),
    limitations: opts.target === "x86_64-apple-darwin" ? [X64_LIMITATION] : [],
  };
}

const USAGE = `usage:
  darwin-artifact-contract.mjs record-check --file F --name N --exit-code C --output-file O [--expect S] -- CMD...
  darwin-artifact-contract.mjs write-receipt --target T --version V [--tag TAG] --commit SHA --tree SHA
      --repo-root DIR --out-dir DIR --binary FILE --checks FILE --uname-m M --os-version S
      --hw-arm64 N --translated N --rustc-vv S --cargo-v S --file-binary S --file-addon S [--cargo-build-jobs N]
  darwin-artifact-contract.mjs asset-names --version V --target T`;

export function main(argv) {
  const [command, ...rest] = argv;
  const { flags, rest: tail } = parseFlags(rest);
  if (command === "record-check") {
    const entry = recordCheck({
      file: need(flags, "file"),
      name: need(flags, "name"),
      exitCode: Number(need(flags, "exit-code")),
      output: readFileSync(need(flags, "output-file"), "utf8"),
      expect: flags.expect,
      command: tail,
    });
    process.stdout.write(`check ${entry.name}: ${entry.first_result}\n`);
    return 0;
  }
  if (command === "write-receipt") {
    const target = need(flags, "target");
    const version = need(flags, "version");
    const receipt = buildReceipt({
      target,
      version,
      tag: flags.tag,
      commit: need(flags, "commit"),
      tree: need(flags, "tree"),
      repoRoot: need(flags, "repo-root"),
      outDir: need(flags, "out-dir"),
      binary: need(flags, "binary"),
      checksFile: need(flags, "checks"),
      unameM: need(flags, "uname-m"),
      osVersion: need(flags, "os-version"),
      hwArm64: Number(need(flags, "hw-arm64")),
      translated: need(flags, "translated"),
      rustcVV: need(flags, "rustc-vv"),
      cargoV: need(flags, "cargo-v"),
      fileBinary: need(flags, "file-binary"),
      fileAddon: need(flags, "file-addon"),
      cargoBuildJobs: flags["cargo-build-jobs"],
    });
    const out = join(need(flags, "out-dir"), assetNames(version, target).receipt);
    writeFileSync(out, `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${basename(out)}: qualification ${receipt.qualification.status}\n`);
    return receipt.qualification.status === "passed" ? 0 : 1;
  }
  if (command === "asset-names") {
    const names = assetNames(need(flags, "version"), need(flags, "target"));
    process.stdout.write(`${Object.values(names).join("\n")}\n`);
    return 0;
  }
  process.stderr.write(`${USAGE}\n`);
  return 2;
}

const invokedPath = process.argv[1] && realpathSync(process.argv[1]);
if (invokedPath && invokedPath === realpathSync(fileURLToPath(import.meta.url))) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`darwin-artifact-contract: ${error.message}\n`);
    process.exitCode = 2;
  }
}
