import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import {
  MH_DYLIB,
  MH_EXECUTE,
  admitDarwinArtifacts,
  parseTarGz,
} from "../admit-darwin-artifacts.mjs";
import { assetNames } from "../darwin-artifact-contract.mjs";
import {
  ARM,
  CPU,
  VERSION,
  X64,
  cleanupTmp,
  createSource,
  fixture,
  machO,
  makeTarGz,
  writeChecksum,
} from "./helpers/darwin-fixture.mjs";

let source;
beforeAll(() => {
  source = createSource();
});
afterAll(() => rmSync(source.repo, { recursive: true, force: true }));
afterEach(cleanupTmp);

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
    const verdict = admit(fixture(source));
    expect(verdict.errors).toEqual([]);
    expect(verdict.admitted).toBe(true);
    expect(Object.keys(verdict.targets).sort()).toEqual([ARM, X64]);
    expect(verdict.targets[X64].arch_label).toBe("rosetta-translated");
    expect(verdict.targets[ARM].binary.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("rejects a missing receipt", () => {
    const dir = fixture(source);
    rmSync(join(dir, assetNames(VERSION, X64).receipt));
    expectRejected(admit(dir), /x86_64-apple-darwin: missing .*qualification\.json/);
  });

  it("rejects a missing addon", () => {
    const dir = fixture(source);
    rmSync(join(dir, assetNames(VERSION, ARM).addon));
    expectRejected(admit(dir), /missing zfb-css-9\.8\.7-aarch64-apple-darwin\.node/);
  });

  it("rejects a wrong source commit, tree or version", () => {
    const dir = fixture(source);
    expectRejected(admit(dir, { sourceCommit: "a".repeat(40) }), /source\.commit/);
    expectRejected(admit(dir, { sourceTree: "b".repeat(40) }), /source\.tree/);
    const wrong = fixture(source, { [ARM]: { receipt: (r) => ({ ...r, version: "9.8.6" }) } });
    expectRejected(admit(wrong), /version 9\.8\.6 != 9\.8\.7/);
  });

  it("rejects a receipt for the wrong target", () => {
    const dir = fixture(source, { [ARM]: { receipt: (r) => ({ ...r, target: X64 }) } });
    expectRejected(admit(dir), /aarch64-apple-darwin: receipt target x86_64-apple-darwin/);
  });

  it("rejects archive, binary and addon digest mismatches", () => {
    const archiveDir = fixture(source, {
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
    const binaryDir = fixture(source, {
      [X64]: {
        receipt: (r) => ({
          ...r,
          artifacts: { ...r.artifacts, binary: { ...r.artifacts.binary, sha256: "1".repeat(64) } },
        }),
      },
    });
    expectRejected(admit(binaryDir), /artifacts\.binary\.sha256 does not match/);
    const addonDir = fixture(source);
    const addonName = assetNames(VERSION, ARM).addon;
    writeFileSync(join(addonDir, addonName), machO(CPU[ARM], MH_DYLIB, "swapped"));
    writeChecksum(addonDir, addonName);
    expectRejected(admit(addonDir), /artifacts\.addon\.sha256 does not match/);
  });

  it("rejects a malformed or mismatched checksum file", () => {
    const dir = fixture(source);
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
      expectRejected(admit(fixture(source, { [ARM]: { archive: makeTarGz(entries) } })), pattern);
    }
  });

  it("rejects a Cargo.lock digest that differs from the source", () => {
    const dir = fixture(source);
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
      admit(fixture(source, { [X64]: { receipt: failCheck } })),
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
    expectRejected(admit(fixture(source, { [ARM]: { receipt: retried } })), /pass-on-retry/);
  });

  it("rejects an x64 receipt labelled native or untranslated", () => {
    const native = (r) => ({ ...r, host: { ...r.host, arch_label: "native-x86_64" } });
    expectRejected(
      admit(fixture(source, { [X64]: { receipt: native } })),
      /arch_label native-x86_64 != rosetta-translated/,
    );
    const untranslated = (r) => ({ ...r, host: { ...r.host, translated: 0 } });
    expectRejected(
      admit(fixture(source, { [X64]: { receipt: untranslated } })),
      /host\.translated 0 != 1/,
    );
    const claim = (r) => ({
      ...r,
      file_output: { ...r.file_output, binary: "native Intel build" },
    });
    expectRejected(admit(fixture(source, { [X64]: { receipt: claim } })), /claims native Intel/);
  });

  it("rejects an arm64 Mach-O in the x64 slot", () => {
    const binary = machO(CPU[ARM], MH_EXECUTE);
    expectRejected(
      admit(fixture(source, { [X64]: { binary } })),
      /x86_64-apple-darwin: binary Mach-O CPU type 0x100000c/,
    );
    const fat = Buffer.alloc(64);
    fat.writeUInt32BE(0xcafebabe, 0);
    expectRejected(
      admit(fixture(source, { [X64]: { addon: fat } })),
      /addon is a fat\/universal Mach-O/,
    );
  });

  it("rejects a conflicting duplicate receipt", () => {
    const dir = fixture(source);
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
    const dir = fixture(source);
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
