import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import {
  MH_DYLIB,
  MH_EXECUTE,
  admitDarwinArtifacts,
  parseTarGz,
  sourceInputs,
} from "../admit-darwin-artifacts.mjs";
import {
  DARWIN_TARGETS,
  assetNames,
  buildReceipt,
  recordCheck,
  sha256,
} from "../darwin-artifact-contract.mjs";

const scriptsDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const VERSION = "9.8.7";
const ARM = "aarch64-apple-darwin";
const X64 = "x86_64-apple-darwin";
const CPU = { [ARM]: 0x0100000c, [X64]: 0x01000007 };
const roots = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function tmp(prefix) {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  roots.push(dir);
  return dir;
}

function machO(cpuType, fileType, tag = "") {
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

function makeTarGz(entries) {
  const parts = [];
  for (const { name, data = Buffer.alloc(0), mode = 0o755, typeflag = "0" } of entries) {
    parts.push(tarHeader(name, data.length, mode, typeflag), data);
    parts.push(Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

// One shared source repository: git work is the only slow part.
let source;
afterAll(() => rmSync(source.repo, { recursive: true, force: true }));
beforeAll(() => {
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
  source = { repo, commit, tree, inputs: sourceInputs(repo, commit, tree).inputs };
});

function writeChecksum(dir, name) {
  writeFileSync(join(dir, `${name}.sha256`), `${sha256(readFileSync(join(dir, name)))}  ${name}\n`);
}

function writeTarget(dir, target, overrides = {}) {
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

function fixture(overrides = {}) {
  const dir = tmp("zfb-admit-assets-");
  writeTarget(dir, ARM, overrides[ARM]);
  writeTarget(dir, X64, overrides[X64]);
  return dir;
}

function admit(dir, extra = {}) {
  return admitDarwinArtifacts({
    dir,
    version: VERSION,
    sourceCommit: source.commit,
    sourceTree: source.tree,
    inputs: source.inputs,
    ...extra,
  });
}

function expectRejected(verdict, pattern) {
  expect(verdict.admitted).toBe(false);
  expect(verdict.errors.join("\n")).toMatch(pattern);
}

describe("admitDarwinArtifacts", () => {
  it("admits a complete, consistent pair and reports digests", () => {
    const verdict = admit(fixture());
    expect(verdict.errors).toEqual([]);
    expect(verdict.admitted).toBe(true);
    expect(Object.keys(verdict.targets).sort()).toEqual([ARM, X64]);
    expect(verdict.targets[X64].arch_label).toBe("rosetta-translated");
    expect(verdict.targets[ARM].binary.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("rejects a missing receipt", () => {
    const dir = fixture();
    rmSync(join(dir, assetNames(VERSION, X64).receipt));
    expectRejected(admit(dir), /x86_64-apple-darwin: missing .*qualification\.json/);
  });

  it("rejects a missing addon", () => {
    const dir = fixture();
    rmSync(join(dir, assetNames(VERSION, ARM).addon));
    expectRejected(admit(dir), /missing zfb-css-9\.8\.7-aarch64-apple-darwin\.node/);
  });

  it("rejects a wrong source commit, tree or version", () => {
    const dir = fixture();
    expectRejected(admit(dir, { sourceCommit: "a".repeat(40) }), /source\.commit/);
    expectRejected(admit(dir, { sourceTree: "b".repeat(40) }), /source\.tree/);
    const wrong = fixture({ [ARM]: { receipt: (r) => ({ ...r, version: "9.8.6" }) } });
    expectRejected(admit(wrong), /version 9\.8\.6 != 9\.8\.7/);
  });

  it("rejects a receipt for the wrong target", () => {
    const dir = fixture({ [ARM]: { receipt: (r) => ({ ...r, target: X64 }) } });
    expectRejected(admit(dir), /aarch64-apple-darwin: receipt target x86_64-apple-darwin/);
  });

  it("rejects archive, binary and addon digest mismatches", () => {
    const archiveDir = fixture({
      [ARM]: {
        receipt: (r) => ({
          ...r,
          artifacts: {
            ...r.artifacts,
            archive: { ...r.artifacts.archive, sha256: "0".repeat(64) },
          },
        }),
      },
    });
    expectRejected(admit(archiveDir), /artifacts\.archive\.sha256 does not match/);
    const binaryDir = fixture({
      [X64]: {
        receipt: (r) => ({
          ...r,
          artifacts: { ...r.artifacts, binary: { ...r.artifacts.binary, sha256: "1".repeat(64) } },
        }),
      },
    });
    expectRejected(admit(binaryDir), /artifacts\.binary\.sha256 does not match/);
    const addonDir = fixture();
    const addonName = assetNames(VERSION, ARM).addon;
    writeFileSync(join(addonDir, addonName), machO(CPU[ARM], MH_DYLIB, "swapped"));
    writeChecksum(addonDir, addonName);
    expectRejected(admit(addonDir), /artifacts\.addon\.sha256 does not match/);
  });

  it("rejects a malformed or mismatched checksum file", () => {
    const dir = fixture();
    const name = assetNames(VERSION, ARM).archive;
    writeFileSync(join(dir, `${name}.sha256`), `${"f".repeat(64)} ${name}\n`);
    expectRejected(admit(dir), /\.sha256 is not one GNU/);
    writeFileSync(join(dir, `${name}.sha256`), `${"f".repeat(64)}  ${name}\n`);
    expectRejected(admit(dir), /archive bytes do not match/);
  });

  it("rejects nested, extra and non-executable archive entries", () => {
    const bin = machO(CPU[ARM], MH_EXECUTE);
    for (const [entries, pattern] of [
      [[{ name: "bin/zfb", data: bin }], /archive entry is "bin\/zfb"/],
      [
        [
          { name: "zfb", data: bin },
          { name: "._zfb", data: Buffer.from("x") },
        ],
        /archive has 2 entries/,
      ],
      [[{ name: "zfb", data: bin, mode: 0o644 }], /mode 644 != 755/],
    ]) {
      expectRejected(admit(fixture({ [ARM]: { archive: makeTarGz(entries) } })), pattern);
    }
  });

  it("rejects a Cargo.lock digest that differs from the source", () => {
    const dir = fixture();
    const inputs = { ...source.inputs, "Cargo.lock": "e".repeat(64) };
    expectRejected(admit(dir, { inputs }), /cargo_lock_sha256 != source Cargo\.lock/);
  });

  it("rejects a failed qualification check", () => {
    const failCheck = (r) => {
      const checks = r.qualification.checks.map((c) =>
        c.name === "css-addon-smoke" ? { ...c, exit_code: 1, first_result: "fail" } : c,
      );
      return { ...r, qualification: { checks, status: "failed" } };
    };
    expectRejected(
      admit(fixture({ [X64]: { receipt: failCheck } })),
      /css-addon-smoke exit_code 1/,
    );
  });

  it("rejects pass-on-retry", () => {
    const retried = (r) => ({
      ...r,
      qualification: {
        ...r.qualification,
        checks: r.qualification.checks.map((c) => ({ ...c, retries: 1 })),
      },
    });
    expectRejected(admit(fixture({ [ARM]: { receipt: retried } })), /pass-on-retry/);
  });

  it("rejects an x64 receipt labelled native or untranslated", () => {
    const native = (r) => ({ ...r, host: { ...r.host, arch_label: "native-x86_64" } });
    expectRejected(
      admit(fixture({ [X64]: { receipt: native } })),
      /arch_label native-x86_64 != rosetta-translated/,
    );
    const untranslated = (r) => ({ ...r, host: { ...r.host, translated: 0 } });
    expectRejected(admit(fixture({ [X64]: { receipt: untranslated } })), /host\.translated 0 != 1/);
    const claim = (r) => ({
      ...r,
      file_output: { ...r.file_output, binary: "native Intel build" },
    });
    expectRejected(admit(fixture({ [X64]: { receipt: claim } })), /claims native Intel/);
  });

  it("rejects an arm64 Mach-O in the x64 slot", () => {
    const binary = machO(CPU[ARM], MH_EXECUTE);
    expectRejected(
      admit(fixture({ [X64]: { binary } })),
      /x86_64-apple-darwin: binary Mach-O CPU type 0x100000c/,
    );
    const fat = Buffer.alloc(64);
    fat.writeUInt32BE(0xcafebabe, 0);
    expectRejected(admit(fixture({ [X64]: { addon: fat } })), /addon is a fat\/universal Mach-O/);
  });

  it("rejects a conflicting duplicate receipt", () => {
    const dir = fixture();
    const receipt = readFileSync(join(dir, assetNames(VERSION, ARM).receipt));
    writeFileSync(join(dir, "zfb-9.8.7-aarch64-apple-darwin.qualification (1).json"), receipt);
    writeFileSync(join(dir, "receipt-copy.json"), receipt);
    const verdict = admit(dir);
    expectRejected(
      verdict,
      /unexpected Darwin asset zfb-9\.8\.7-aarch64-apple-darwin\.qualification \(1\)\.json/,
    );
    expectRejected(verdict, /conflicting qualification receipt receipt-copy\.json/);
  });

  it("ignores non-Darwin assets of the same Release", () => {
    const dir = fixture();
    writeFileSync(join(dir, `zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`), "linux");
    expect(admit(dir).admitted).toBe(true);
  });

  it("parses a single-entry archive", () => {
    const entries = parseTarGz(makeTarGz([{ name: "zfb", data: Buffer.from("abc") }]));
    expect(entries.map((e) => [e.name, e.mode, e.data.toString()])).toEqual([
      ["zfb", 0o755, "abc"],
    ]);
  });
});

describe("admit-darwin-artifacts CLI", () => {
  const cli = join(scriptsDir, "admit-darwin-artifacts.mjs");
  const run = (args) => spawnSync(process.execPath, [cli, ...args], { encoding: "utf8" });
  const base = (dir) => [
    "--dir",
    dir,
    "--version",
    VERSION,
    "--source-commit",
    source.commit,
    "--source-tree",
    source.tree,
    "--repo-root",
    source.repo,
  ];

  it("exits 0 with a verdict file when admitted", () => {
    const dir = fixture();
    const out = join(tmp("zfb-admit-out-"), "verdict.json");
    const result = run([...base(dir), "--json-out", out]);
    expect(result.status).toBe(0);
    expect(JSON.parse(readFileSync(out, "utf8")).admitted).toBe(true);
  });

  it("exits 1 when rejected and 2 on usage errors", () => {
    const dir = fixture();
    rmSync(join(dir, assetNames(VERSION, ARM).archive));
    expect(run(base(dir)).status).toBe(1);
    expect(run(["--dir", dir]).status).toBe(2);
    expect(run([...base(dir).slice(0, -2), "--repo-root", tmp("zfb-admit-nogit-")]).status).toBe(2);
  });
});

describe("build-darwin-local.sh refusals (--check-only, no build)", () => {
  function sandbox() {
    const repo = tmp("zfb-build-darwin-");
    mkdirSync(join(repo, "scripts"));
    mkdirSync(join(repo, "packages", "zfb"), { recursive: true });
    copyFileSync(
      join(scriptsDir, "build-darwin-local.sh"),
      join(repo, "scripts", "build-darwin-local.sh"),
    );
    writeFileSync(
      join(repo, "packages", "zfb", "package.json"),
      JSON.stringify({ version: VERSION }),
    );
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
    git("commit", "-q", "-m", "release");
    git("tag", `v${VERSION}`);
    return {
      repo,
      git,
      head: git("rev-parse", "HEAD"),
      out: join(tmp("zfb-build-out-"), "assets"),
    };
  }
  const run = (repo, args) =>
    spawnSync("bash", [join(repo, "scripts", "build-darwin-local.sh"), ...args], {
      encoding: "utf8",
    });

  it("accepts a clean tree at --source and its tag", () => {
    const { repo, head, out } = sandbox();
    const result = run(repo, [
      "--source",
      head,
      "--tag",
      `v${VERSION}`,
      "--out-dir",
      out,
      "--check-only",
    ]);
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("source binding OK");
  });

  it("refuses a missing or mismatched --source", () => {
    const { repo, out } = sandbox();
    expect(run(repo, ["--check-only"]).stderr).toMatch(/--source <sha> is required/);
    expect(
      run(repo, ["--source", "a".repeat(40), "--out-dir", out, "--check-only"]).stderr,
    ).toMatch(/!= HEAD/);
  });

  it("refuses a tag that is not v<version>, conflicts with --upload, or peels elsewhere", () => {
    const { repo, git, head, out } = sandbox();
    expect(
      run(repo, ["--source", head, "--tag", "v0.0.1", "--out-dir", out, "--check-only"]).stderr,
    ).toMatch(/--tag v0\.0\.1 != v9\.8\.7/);
    expect(
      run(repo, ["--source", head, "--tag", `v${VERSION}`, "--upload", "v1.0.0", "--check-only"])
        .stderr,
    ).toMatch(/conflicts with --tag/);
    const other = git("commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "other");
    git("tag", "-f", `v${VERSION}`, other);
    expect(
      run(repo, ["--source", head, "--tag", `v${VERSION}`, "--out-dir", out, "--check-only"])
        .stderr,
    ).toMatch(/peels to [0-9a-f]{40}, not/);
  });

  it("refuses a dirty tree and an out dir inside the checkout", () => {
    const { repo, head, out } = sandbox();
    expect(
      run(repo, ["--source", head, "--out-dir", join(repo, "inside"), "--check-only"]).stderr,
    ).toMatch(/inside the source checkout/);
    writeFileSync(join(repo, "untracked.txt"), "x");
    expect(run(repo, ["--source", head, "--out-dir", out, "--check-only"]).stderr).toMatch(
      /working tree is dirty/,
    );
  });
});
