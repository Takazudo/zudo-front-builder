// Shared fixtures for the Darwin admission suites: fake thin Mach-O images,
// hand-built ustar archives and receipts produced by the real receipt writer.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { MH_DYLIB, MH_EXECUTE, sourceInputs } from "../../admit-darwin-artifacts.mjs";
import {
  DARWIN_TARGETS,
  assetNames,
  buildReceipt,
  recordCheck,
  sha256,
} from "../../darwin-artifact-contract.mjs";

export const scriptsDir = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
export const VERSION = "9.8.7";
export const ARM = "aarch64-apple-darwin";
export const X64 = "x86_64-apple-darwin";
export const CPU = { [ARM]: 0x0100000c, [X64]: 0x01000007 };
const roots = [];

export function cleanupTmp() {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
}

export function tmp(prefix) {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  roots.push(dir);
  return dir;
}

export function machO(cpuType, fileType, tag = "") {
  const buf = Buffer.alloc(64);
  buf.writeUInt32LE(0xfeedfacf, 0);
  buf.writeUInt32LE(cpuType, 4);
  buf.writeUInt32LE(fileType, 12);
  buf.write(tag, 32);
  return buf;
}

function tarHeader(name, size, mode, typeflag = "0") {
  const h = Buffer.alloc(512);
  h.write(name, 0);
  h.write(`${mode.toString(8).padStart(7, "0")}\0`, 100);
  h.write("0000000\0", 108);
  h.write("0000000\0", 116);
  h.write(`${size.toString(8).padStart(11, "0")}\0`, 124);
  h.write("00000000000\0", 136);
  h.write("        ", 148);
  h.write(typeflag, 156);
  h.write("ustar\0", 257);
  h.write("00", 263);
  let sum = 0;
  for (const b of h) sum += b;
  h.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
  return h;
}

export function makeTarGz(entries) {
  const parts = [];
  for (const { name, data = Buffer.alloc(0), mode = 0o755, typeflag = "0" } of entries) {
    parts.push(tarHeader(name, data.length, mode, typeflag), data);
    parts.push(Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

// A source repository holding the three digest inputs. Callers create one per
// suite (git work is the only slow part) and remove source.repo themselves.
export function createSource() {
  const repo = mkdtempSync(join(tmpdir(), "zfb-admit-src-"));
  writeFileSync(join(repo, "Cargo.lock"), "# lock\n");
  writeFileSync(join(repo, "rust-toolchain.toml"), '[toolchain]\nchannel = "stable"\n');
  writeFileSync(join(repo, "pnpm-lock.yaml"), "lockfileVersion: '9.0'\n");
  const git = (...args) =>
    execFileSync(
      "git",
      ["-C", repo, "-c", "user.email=t@example.invalid", "-c", "user.name=t", ...args],
      {
        encoding: "utf8",
      },
    ).trim();
  git("init", "-q");
  git("add", ".");
  git("commit", "-q", "-m", "source");
  const commit = git("rev-parse", "HEAD");
  const tree = git("rev-parse", "HEAD^{tree}");
  return { repo, commit, tree, inputs: sourceInputs(repo, commit, tree).inputs };
}

export function writeChecksum(dir, name) {
  writeFileSync(join(dir, `${name}.sha256`), `${sha256(readFileSync(join(dir, name)))}  ${name}\n`);
}

function writeTarget(source, dir, target, overrides = {}) {
  const work = tmp("zfb-admit-work-");
  const names = assetNames(VERSION, target);
  const binary = overrides.binary ?? machO(CPU[target], MH_EXECUTE, `zfb ${target}`);
  const archive = overrides.archive ?? makeTarGz([{ name: "zfb", data: binary }]);
  writeFileSync(join(dir, names.archive), archive);
  writeFileSync(
    join(dir, names.addon),
    overrides.addon ?? machO(CPU[target], MH_DYLIB, `css ${target}`),
  );
  writeChecksum(dir, names.archive);
  writeChecksum(dir, names.addon);
  writeFileSync(join(work, "zfb"), binary);
  const spec = DARWIN_TARGETS[target];
  const checks = join(work, "checks.jsonl");
  writeFileSync(checks, "");
  const prefix = target === X64 ? ["arch", "-x86_64"] : ["arch", "-arm64"];
  const outputs = {
    "proc-translated": String(spec.translated),
    "cli-version": `zfb ${VERSION}`,
    "macho-binary-arch": spec.unameM,
    "macho-addon-arch": spec.unameM,
    "node-identity": target === X64 ? "x64" : "arm64",
  };
  for (const name of spec.requiredChecks) {
    const outFile = join(work, `${name}.out`);
    writeFileSync(outFile, outputs[name] ?? "ok");
    recordCheck({
      file: checks,
      name,
      exitCode: 0,
      output: readFileSync(outFile, "utf8"),
      expect: outputs[name],
      command: name.startsWith("macho") ? ["lipo", "-archs", "x"] : [...prefix, "cmd"],
    });
  }
  const receipt = buildReceipt({
    target,
    version: VERSION,
    tag: `v${VERSION}`,
    commit: source.commit,
    tree: source.tree,
    repoRoot: source.repo,
    outDir: dir,
    binary: join(work, "zfb"),
    checksFile: checks,
    unameM: spec.unameM,
    osVersion: "macOS 27.0 (test)",
    hwArm64: 1,
    translated: String(spec.translated),
    rustcVV: "rustc 1.99.0\nhost: aarch64-apple-darwin",
    cargoV: "cargo 1.99.0",
    fileBinary: "Mach-O 64-bit executable",
    fileAddon: "Mach-O 64-bit dynamically linked shared library",
  });
  writeFileSync(join(dir, names.receipt), JSON.stringify(overrides.receipt?.(receipt) ?? receipt));
}

export function fixture(source, overrides = {}) {
  const dir = tmp("zfb-admit-assets-");
  writeTarget(source, dir, ARM, overrides[ARM]);
  writeTarget(source, dir, X64, overrides[X64]);
  return dir;
}
