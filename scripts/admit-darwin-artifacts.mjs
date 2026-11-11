#!/usr/bin/env node
// Linux-runnable admission verifier for owner-Mac Darwin release inputs (#4689).
// It runs before any irreversible release step and admits the two Darwin
// targets only when every byte, receipt field and source binding agrees.
// Contract: scripts/darwin-artifact-contract.md. Node builtins only.
//
// Exit codes: 0 admitted, 1 rejected, 2 usage or I/O error.
import { execFileSync } from "node:child_process";
import {
  existsSync,
  lstatSync,
  readFileSync,
  readdirSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import {
  DARWIN_TARGETS,
  INPUT_FILES,
  OBJECT_ID_RE,
  RECEIPT_SCHEMA,
  SEMVER_RE,
  SHA256_RE,
  VERDICT_SCHEMA,
  X64_LIMITATION,
  assetNames,
  sha256,
} from "./darwin-artifact-contract.mjs";

export class UsageError extends Error {}

// ── tar.gz ───────────────────────────────────────────────────────────────────

function cString(buf, start, length) {
  const slice = buf.subarray(start, start + length);
  const nul = slice.indexOf(0);
  return slice.subarray(0, nul === -1 ? slice.length : nul).toString("utf8");
}

function octal(buf, start, length, label) {
  const text = cString(buf, start, length).trim();
  if (!/^[0-7]+$/.test(text))
    throw new Error(`tar header ${label} is not octal: ${JSON.stringify(text)}`);
  return Number.parseInt(text, 8);
}

function parsePax(data) {
  const records = {};
  let offset = 0;
  const text = data.toString("utf8");
  while (offset < text.length) {
    const space = text.indexOf(" ", offset);
    if (space === -1) throw new Error("malformed pax record");
    const length = Number(text.slice(offset, space));
    if (!Number.isInteger(length) || length <= 0) throw new Error("malformed pax record length");
    const record = text.slice(space + 1, offset + length - 1);
    const eq = record.indexOf("=");
    if (eq === -1) throw new Error("malformed pax record");
    records[record.slice(0, eq)] = record.slice(eq + 1);
    offset += length;
  }
  return records;
}

// Returns the regular-file entries of a gzip'd ustar/pax archive. Per-file pax
// headers ('x') are folded into the next entry; every other non-regular entry
// type is reported so the caller can reject it.
export function parseTarGz(bytes) {
  const tar = gunzipSync(bytes);
  const entries = [];
  let offset = 0;
  let pax = null;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    let checksum = 0;
    for (let i = 0; i < 512; i++) checksum += i >= 148 && i < 156 ? 32 : header[i];
    if (checksum !== octal(header, 148, 8, "checksum"))
      throw new Error("tar header checksum mismatch");
    const typeflag = String.fromCharCode(header[156] || 48);
    let size = octal(header, 124, 12, "size");
    const prefix = cString(header, 345, 155);
    let name = cString(header, 0, 100);
    if (prefix) name = `${prefix}/${name}`;
    const mode = octal(header, 100, 8, "mode");
    const dataStart = offset + 512;
    if (pax?.size !== undefined) size = Number(pax.size);
    if (dataStart + size > tar.length) throw new Error("tar entry truncated");
    const data = tar.subarray(dataStart, dataStart + size);
    offset = dataStart + Math.ceil(size / 512) * 512;
    if (typeflag === "x") {
      pax = parsePax(data);
      continue;
    }
    if (pax?.path !== undefined) name = pax.path;
    pax = null;
    entries.push({ name, typeflag, mode, size, data });
  }
  if (pax) throw new Error("dangling pax header");
  return entries;
}

// ── Mach-O ───────────────────────────────────────────────────────────────────

const MH_MAGIC_64 = 0xfeedfacf;
const FAT_MAGICS = new Set([0xcafebabe, 0xbebafeca, 0xcafebabf, 0xbfbafeca]);
export const MH_EXECUTE = 0x2;
export const MH_DYLIB = 0x6;
export const MH_BUNDLE = 0x8;

export function readMachO(bytes) {
  if (bytes.length < 16) return { kind: "unknown" };
  const be = bytes.readUInt32BE(0);
  if (FAT_MAGICS.has(be)) return { kind: "fat" };
  if (bytes.readUInt32LE(0) !== MH_MAGIC_64) return { kind: "unknown" };
  return { kind: "thin", cpuType: bytes.readUInt32LE(4), fileType: bytes.readUInt32LE(12) };
}

function checkMachO(errors, label, bytes, target, fileTypes) {
  const info = readMachO(bytes);
  const want = DARWIN_TARGETS[target].cpuType;
  if (info.kind === "fat") {
    errors.push(
      `${target}: ${label} is a fat/universal Mach-O; only a thin ${target} image is admitted`,
    );
  } else if (info.kind !== "thin") {
    errors.push(`${target}: ${label} is not a 64-bit little-endian Mach-O`);
  } else {
    if (info.cpuType !== want) {
      errors.push(
        `${target}: ${label} Mach-O CPU type 0x${info.cpuType.toString(16)} != 0x${want.toString(16)}`,
      );
    }
    if (!fileTypes.includes(info.fileType)) {
      errors.push(`${target}: ${label} Mach-O file type ${info.fileType} not in [${fileTypes}]`);
    }
  }
}

// ── checksum files ──────────────────────────────────────────────────────────

export function parseChecksumFile(text, expectedName) {
  const match = /^([0-9a-f]{64}) {2}([^\n/]+)\n?$/.exec(text);
  if (!match) return { error: "is not one GNU '<sha256>  <basename>' line" };
  if (match[2] !== expectedName) return { error: `names ${match[2]}, expected ${expectedName}` };
  return { sha256: match[1] };
}

// ── source binding ───────────────────────────────────────────────────────────

function git(repoRoot, args, encoding = "utf8") {
  return execFileSync("git", ["-C", repoRoot, ...args], {
    encoding,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
  });
}

// Digests come from the git objects at the source commit, never the working
// tree, so a dirty or differently checked-out repo cannot launder an input.
export function sourceInputs(repoRoot, commit, tree) {
  let resolved;
  let actualTree;
  try {
    resolved = git(repoRoot, ["rev-parse", "--verify", `${commit}^{commit}`]).trim();
    actualTree = git(repoRoot, ["rev-parse", "--verify", `${commit}^{tree}`]).trim();
  } catch (error) {
    throw new UsageError(
      `cannot resolve ${commit} in ${repoRoot}: ${error.stderr || error.message}`,
    );
  }
  const errors = [];
  if (resolved !== commit) errors.push(`source commit ${commit} resolves to ${resolved}`);
  if (actualTree !== tree)
    errors.push(`source commit ${commit} has tree ${actualTree}, not ${tree}`);
  const inputs = {};
  for (const file of INPUT_FILES) {
    try {
      inputs[file] = sha256(git(repoRoot, ["show", `${commit}:${file}`], "buffer"));
    } catch (error) {
      throw new UsageError(`cannot read ${file} at ${commit}: ${error.stderr || error.message}`);
    }
  }
  return { errors, inputs };
}

// ── receipt ──────────────────────────────────────────────────────────────────

function checkReceipt(errors, receipt, ctx) {
  const { target, version, sourceCommit, sourceTree, inputs, actual } = ctx;
  const spec = DARWIN_TARGETS[target];
  const fail = (msg) => errors.push(`${target}: receipt ${msg}`);
  if (receipt?.schema !== RECEIPT_SCHEMA)
    return fail(`schema is ${JSON.stringify(receipt?.schema)}`);
  if (receipt.version !== version) fail(`version ${receipt.version} != ${version}`);
  if (receipt.target !== target) fail(`target ${receipt.target} != ${target}`);
  if (receipt.source?.commit !== sourceCommit)
    fail(`source.commit ${receipt.source?.commit} != ${sourceCommit}`);
  if (receipt.source?.tree !== sourceTree)
    fail(`source.tree ${receipt.source?.tree} != ${sourceTree}`);
  if (receipt.tag !== null && receipt.tag !== undefined && receipt.tag !== `v${version}`) {
    fail(`tag ${receipt.tag} != v${version}`);
  }

  const host = receipt.host ?? {};
  if (host.arch_label !== spec.archLabel)
    fail(`host.arch_label ${host.arch_label} != ${spec.archLabel}`);
  if (host.translated !== spec.translated)
    fail(`host.translated ${host.translated} != ${spec.translated}`);
  if (host.uname_m !== spec.unameM) fail(`host.uname_m ${host.uname_m} != ${spec.unameM}`);
  if (host.hw_optional_arm64 !== 1)
    fail(`host.hw_optional_arm64 ${host.hw_optional_arm64} != 1 (Apple Silicon host)`);
  if (target === "x86_64-apple-darwin") {
    // The mandated limitation itself says "no native Intel ... claim", so it is
    // excluded from the scan; everything else must never claim native Intel.
    const claims = JSON.stringify({ ...receipt, limitations: undefined }).toLowerCase();
    if (/native[ -_]?(intel|x86|x64)/.test(claims)) fail("claims native Intel/x86_64 execution");
    if (!Array.isArray(receipt.limitations) || !receipt.limitations.includes(X64_LIMITATION)) {
      fail("limitations lack the Rosetta functional-only statement");
    }
  }

  const tc = receipt.toolchain ?? {};
  if (tc.profile !== "release") fail(`toolchain.profile ${tc.profile} != release`);
  if (tc.env?.ZFB_RELEASE_VERSION !== version)
    fail(`toolchain.env.ZFB_RELEASE_VERSION != ${version}`);
  if (typeof tc.rustc_vV !== "string" || !tc.rustc_vV.includes("host:"))
    fail("toolchain.rustc_vV missing");
  if (typeof tc.cargo_V !== "string" || !tc.cargo_V.startsWith("cargo "))
    fail("toolchain.cargo_V missing");
  if (tc.cargo_lock_sha256 !== inputs["Cargo.lock"])
    fail("toolchain.cargo_lock_sha256 != source Cargo.lock");
  if (tc.rust_toolchain_toml_sha256 !== inputs["rust-toolchain.toml"]) {
    fail("toolchain.rust_toolchain_toml_sha256 != source rust-toolchain.toml");
  }
  for (const file of INPUT_FILES) {
    if (receipt.inputs?.[file] !== inputs[file]) fail(`inputs[${file}] != source ${file} digest`);
  }

  for (const [key, want] of Object.entries(actual)) {
    const got = receipt.artifacts?.[key];
    if (!got) {
      fail(`artifacts.${key} missing`);
      continue;
    }
    if (got.name !== want.name) fail(`artifacts.${key}.name ${got.name} != ${want.name}`);
    if (got.sha256 !== want.sha256) fail(`artifacts.${key}.sha256 does not match the actual bytes`);
    if (got.size !== want.size) fail(`artifacts.${key}.size ${got.size} != ${want.size}`);
  }
  for (const key of ["binary", "addon"]) {
    if (typeof receipt.file_output?.[key] !== "string" || !receipt.file_output[key]) {
      fail(`file_output.${key} missing`);
    }
  }

  const q = receipt.qualification ?? {};
  if (q.status !== "passed") fail(`qualification.status ${q.status}`);
  const checks = Array.isArray(q.checks) ? q.checks : [];
  const seen = new Map();
  for (const check of checks) {
    if (seen.has(check.name)) fail(`qualification check ${check.name} appears twice`);
    seen.set(check.name, check);
    if (check.exit_code !== 0) fail(`check ${check.name} exit_code ${check.exit_code}`);
    if (check.first_result !== "pass")
      fail(`check ${check.name} first_result ${check.first_result}`);
    if (check.retries !== 0)
      fail(`check ${check.name} retries ${check.retries} (pass-on-retry is not passed)`);
    if (!Array.isArray(check.command) || check.command.length === 0)
      fail(`check ${check.name} has no command`);
  }
  for (const name of spec.requiredChecks) {
    if (!seen.has(name)) fail(`required check ${name} missing`);
  }
  const versionCheck = seen.get("cli-version");
  if (versionCheck && versionCheck.output !== `zfb ${version}`) {
    fail(`cli-version output ${JSON.stringify(versionCheck.output)} != "zfb ${version}"`);
  }
  const translatedCheck = seen.get("proc-translated");
  if (translatedCheck && translatedCheck.output !== String(spec.translated)) {
    fail(
      `proc-translated output ${JSON.stringify(translatedCheck.output)} != "${spec.translated}"`,
    );
  }
  if (target === "x86_64-apple-darwin") {
    for (const name of [
      "proc-translated",
      "cli-version",
      "cli-help",
      "node-identity",
      "css-addon-smoke",
    ]) {
      const command = seen.get(name)?.command;
      if (command && !(command[0] === "arch" && command[1] === "-x86_64")) {
        fail(`check ${name} did not run under arch -x86_64`);
      }
    }
  }
  if (!SHA256_RE.test(receipt.artifacts?.archive?.sha256 ?? ""))
    fail("artifacts.archive.sha256 malformed");
  if (Number.isNaN(Date.parse(receipt.created_at))) fail("created_at is not a timestamp");
}

// ── admission ────────────────────────────────────────────────────────────────

function readRegular(dir, name, errors, target) {
  const path = join(dir, name);
  if (!existsSync(path)) {
    errors.push(`${target}: missing ${name}`);
    return null;
  }
  const stat = lstatSync(path);
  if (!stat.isFile()) {
    errors.push(`${target}: ${name} is not a regular file`);
    return null;
  }
  return readFileSync(path);
}

export function admitDarwinArtifacts({ dir, version, sourceCommit, sourceTree, inputs }) {
  const errors = [];
  const warnings = [];
  const targets = {};
  if (!SEMVER_RE.test(version)) throw new UsageError(`--version ${version} is not semver`);
  if (!OBJECT_ID_RE.test(sourceCommit))
    throw new UsageError("--source-commit must be a full hex object id");
  if (!OBJECT_ID_RE.test(sourceTree))
    throw new UsageError("--source-tree must be a full hex object id");
  let listing;
  try {
    listing = readdirSync(dir);
  } catch (error) {
    throw new UsageError(`cannot read --dir ${dir}: ${error.message}`);
  }

  // A Release asset name is unique, so any extra Darwin-tagged file is a
  // deliberate second candidate for the same slot. Ambiguity about which bytes
  // are authoritative is rejected, not warned about. Non-Darwin assets of the
  // same Release are ignored.
  const expected = new Set();
  for (const target of Object.keys(DARWIN_TARGETS)) {
    for (const name of Object.values(assetNames(version, target))) expected.add(name);
  }
  for (const name of listing) {
    const darwinTagged = Object.keys(DARWIN_TARGETS).some((t) => name.includes(t));
    if (darwinTagged && !expected.has(name)) {
      errors.push(`unexpected Darwin asset ${name} (duplicate or conflicting candidate)`);
    }
  }
  for (const name of listing.filter((n) => n.endsWith(".json") && !expected.has(n))) {
    try {
      const parsed = JSON.parse(readFileSync(join(dir, name), "utf8"));
      if (parsed?.schema === RECEIPT_SCHEMA) {
        errors.push(`conflicting qualification receipt ${name} (target ${parsed.target})`);
      }
    } catch {
      // Not a JSON receipt; other Release assets are out of scope.
    }
  }

  for (const target of Object.keys(DARWIN_TARGETS)) {
    const names = assetNames(version, target);
    const before = errors.length;
    const archive = readRegular(dir, names.archive, errors, target);
    const archiveSumFile = readRegular(dir, names.archiveSha256, errors, target);
    const addon = readRegular(dir, names.addon, errors, target);
    const addonSumFile = readRegular(dir, names.addonSha256, errors, target);
    const receiptBytes = readRegular(dir, names.receipt, errors, target);
    if (errors.length > before) continue;

    for (const [label, sumBytes, bytes, name] of [
      ["archive", archiveSumFile, archive, names.archive],
      ["addon", addonSumFile, addon, names.addon],
    ]) {
      const parsed = parseChecksumFile(sumBytes.toString("utf8"), name);
      if (parsed.error) errors.push(`${target}: ${name}.sha256 ${parsed.error}`);
      else if (parsed.sha256 !== sha256(bytes))
        errors.push(`${target}: ${label} bytes do not match ${name}.sha256`);
    }

    let binary = null;
    try {
      const entries = parseTarGz(archive);
      if (entries.length !== 1) {
        errors.push(
          `${target}: archive has ${entries.length} entries (${entries.map((e) => e.name).join(", ")}); exactly one root file "zfb" is required`,
        );
      } else {
        const [entry] = entries;
        if (entry.name !== "zfb")
          errors.push(`${target}: archive entry is ${JSON.stringify(entry.name)}, not "zfb"`);
        if (!["0", "\0"].includes(entry.typeflag))
          errors.push(`${target}: archive entry is not a regular file (type ${entry.typeflag})`);
        if ((entry.mode & 0o7777) !== 0o755)
          errors.push(`${target}: archive entry mode ${(entry.mode & 0o7777).toString(8)} != 755`);
        binary = Buffer.from(entry.data);
      }
    } catch (error) {
      errors.push(`${target}: archive unreadable: ${error.message}`);
    }

    if (binary) checkMachO(errors, "binary", binary, target, [MH_EXECUTE]);
    checkMachO(errors, "addon", addon, target, [MH_DYLIB, MH_BUNDLE]);

    let receipt;
    try {
      receipt = JSON.parse(receiptBytes.toString("utf8"));
    } catch (error) {
      errors.push(`${target}: receipt is not JSON: ${error.message}`);
      continue;
    }
    const actual = {
      archive: { name: names.archive, sha256: sha256(archive), size: archive.length },
      archive_sha256_file: {
        name: names.archiveSha256,
        sha256: sha256(archiveSumFile),
        size: archiveSumFile.length,
      },
      addon: { name: names.addon, sha256: sha256(addon), size: addon.length },
      addon_sha256_file: {
        name: names.addonSha256,
        sha256: sha256(addonSumFile),
        size: addonSumFile.length,
      },
    };
    if (binary) actual.binary = { name: "zfb", sha256: sha256(binary), size: binary.length };
    checkReceipt(errors, receipt, { target, version, sourceCommit, sourceTree, inputs, actual });

    targets[target] = {
      arch_label: receipt?.host?.arch_label ?? null,
      receipt: { name: names.receipt, sha256: sha256(receiptBytes) },
      ...actual,
    };
  }

  return {
    schema: VERDICT_SCHEMA,
    admitted: errors.length === 0,
    version,
    source: { commit: sourceCommit, tree: sourceTree },
    inputs,
    targets,
    errors,
    warnings,
  };
}

const USAGE =
  "usage: admit-darwin-artifacts.mjs --dir DIR --version V --source-commit SHA --source-tree SHA --repo-root DIR [--json-out FILE]";

export function parseArgs(argv) {
  const opts = {};
  const keys = new Set(["dir", "version", "source-commit", "source-tree", "repo-root", "json-out"]);
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    if (!argv[i]?.startsWith("--") || !keys.has(key))
      throw new UsageError(`unknown argument ${argv[i]}`);
    if (argv[i + 1] === undefined) throw new UsageError(`${argv[i]} needs a value`);
    if (opts[key] !== undefined) throw new UsageError(`${argv[i]} given twice`);
    opts[key] = argv[i + 1];
  }
  for (const key of ["dir", "version", "source-commit", "source-tree", "repo-root"]) {
    if (!opts[key]) throw new UsageError(`missing --${key}`);
  }
  return opts;
}

export function main(argv) {
  let verdict;
  let opts;
  try {
    opts = parseArgs(argv);
    if (!OBJECT_ID_RE.test(opts["source-commit"]))
      throw new UsageError("--source-commit must be a full hex object id");
    const source = sourceInputs(opts["repo-root"], opts["source-commit"], opts["source-tree"]);
    verdict = admitDarwinArtifacts({
      dir: opts.dir,
      version: opts.version,
      sourceCommit: opts["source-commit"],
      sourceTree: opts["source-tree"],
      inputs: source.inputs,
    });
    verdict.errors.unshift(...source.errors);
    verdict.admitted = verdict.errors.length === 0;
  } catch (error) {
    if (error instanceof UsageError) {
      process.stderr.write(`admit-darwin-artifacts: ${error.message}\n${USAGE}\n`);
      return 2;
    }
    throw error;
  }
  const json = `${JSON.stringify(verdict, null, 2)}\n`;
  process.stdout.write(json);
  if (opts["json-out"]) {
    try {
      writeFileSync(opts["json-out"], json);
    } catch (error) {
      process.stderr.write(`admit-darwin-artifacts: cannot write --json-out: ${error.message}\n`);
      return 2;
    }
  }
  for (const error of verdict.errors) process.stderr.write(`REJECT: ${error}\n`);
  return verdict.admitted ? 0 : 1;
}

const invokedPath = process.argv[1] && realpathSync(process.argv[1]);
if (invokedPath && invokedPath === realpathSync(fileURLToPath(import.meta.url))) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`admit-darwin-artifacts: ${error.stack || error.message}\n`);
    process.exitCode = 2;
  }
}
