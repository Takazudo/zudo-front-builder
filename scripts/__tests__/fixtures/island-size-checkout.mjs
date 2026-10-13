// Disposable checkouts and synthetic measurement runs for the island-size
// rebaseline helpers' end-to-end tests (#4193). The checkout copies only what
// the helpers read: lockfiles, the fingerprinted importer manifests, the size
// fixtures/runners, the budget test file and an empty README.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { computeLockFingerprint } from "../../../research/v3-island-size/lock-fingerprint.mjs";
import {
  NORMALIZED_SIDECAR,
  normalizeFixtureFiles,
  normalizeMeasurementDirectory,
} from "../../../research/v3-island-size/normalize-identity.mjs";

export const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const SIZE_DIR = "research/v3-island-size";
const MODES = ["workspace", "packed"];

export function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

function copy(root, relative) {
  const from = join(repoRoot, relative);
  if (!existsSync(from)) return;
  mkdirSync(dirname(join(root, relative)), { recursive: true });
  copyFileSync(from, join(root, relative));
}

// The live contracts are schemaVersion 3 since #4194; cases that exercise the
// schemaVersion-2 paths start from the same contract without its fingerprint.
export function asSchemaV2(contract) {
  const v2 = structuredClone(contract);
  delete v2.dependencyFingerprint;
  v2.schemaVersion = 2;
  return v2;
}

export function write(root, relative, bytes) {
  const path = join(root, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
}

// Returns the checkout root and the identity a native run of it would record.
// `cargoLock` replaces the copied Cargo.lock with the given text.
export function makeCheckout(roots, { cargoLock } = {}) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-checkout-"));
  roots.push(root);
  const lockText = readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
  const version = JSON.parse(readFileSync(join(repoRoot, "packages/zfb/package.json"))).version;
  const { inventory } = computeLockFingerprint({ lockText, currentVersion: version });
  for (const relative of [
    "pnpm-lock.yaml",
    "Cargo.lock",
    "tsconfig.base.json",
    "scripts/__tests__/island-size-budget.test.mjs",
    SIZE_DIR + "/measure-real.mjs",
    SIZE_DIR + "/measure.mjs",
    ...inventory.importers.flatMap((importer) => [
      importer + "/package.json",
      importer + "/tsconfig.json",
    ]),
  ]) {
    copy(root, relative);
  }
  copy(root, "packages/zfb/package.json");
  if (cargoLock !== undefined) write(root, "Cargo.lock", cargoLock);
  const fixtureSha256 = {};
  for (const name of [
    "event-only",
    "scalar-signal",
    "show-for",
    "model",
    "blog-menu",
    "theme-toggle",
    "json-api",
  ]) {
    copy(root, SIZE_DIR + "/fixtures/" + name + ".tsx");
    fixtureSha256[name] = sha(readFileSync(join(root, SIZE_DIR, "fixtures", name + ".tsx")));
  }
  write(root, SIZE_DIR + "/README.md", "# island size\n");
  const fingerprint = computeLockFingerprint({ lockText, currentVersion: version, repoRoot: root });
  return {
    root,
    version,
    pnpmLockSha256: sha(readFileSync(join(root, "pnpm-lock.yaml"))),
    cargoLockSha256: sha(readFileSync(join(root, "Cargo.lock"))),
    fixtureSha256,
    runnerSha256: Object.fromEntries(
      ["measure-real.mjs", "measure.mjs"].map((name) => [
        name,
        sha(readFileSync(join(root, SIZE_DIR, name))),
      ]),
    ),
    dependencyFingerprint: {
      policyVersion: fingerprint.policyVersion,
      parser: fingerprint.parser,
      helperSha256: fingerprint.helperSha256,
      sha256: fingerprint.sha256,
    },
  };
}

// The saved dist of one fixture: a 16-hex identity token at both compiler sites.
export function fixtureFiles(mode, fixture) {
  const token = sha(mode + fixture).slice(0, 16);
  if (fixture === "no-island") {
    return { "index.html": Buffer.from("<!doctype html><body>empty</body>\n") };
  }
  const entry =
    "// " +
    fixture +
    '\nfunction r(n){return{identity:{component:n,build:"' +
    token +
    '"},mount:()=>{let l={component:n,build:"' +
    token +
    '"};return l}}}\nexport {};\n';
  return {
    "index.html": Buffer.from(
      '<!doctype html><script type="module" src="/assets/islands-x.js"></script><div data-zfb-build="' +
        token +
        '"></div>\n',
    ),
    "assets/islands-x.js": Buffer.from(entry),
  };
}

// Writes `real-<mode>/` with two identical passes per fixture, a measurement.json
// carrying `provenanceFor(mode)` and the normalizer sidecar.
export function writeRuns(root, fixtures, provenanceFor) {
  for (const mode of MODES) {
    const results = {};
    for (const fixture of fixtures) {
      const files = fixtureFiles(mode, fixture);
      for (const pass of [1, 2]) {
        for (const [name, bytes] of Object.entries(files)) {
          write(root, "real-" + mode + "/" + fixture + "/pass-" + pass + "/" + name, bytes);
        }
      }
      const actual = normalizeFixtureFiles(files, mode + " " + fixture);
      results[fixture] = {
        chunks: actual.chunks.map(({ file, raw, gzip, sha256, phase, role }) => ({
          file,
          raw,
          gzip,
          sha256,
          phase,
          role,
        })),
        initial: { raw: actual.initial.raw, gzip: actual.initial.gzip },
        later: { raw: actual.later.raw, gzip: actual.later.gzip },
        shipped: { raw: actual.shipped.raw, gzip: actual.shipped.gzip },
      };
    }
    const path = join(root, "real-" + mode, "measurement.json");
    writeFileSync(path, JSON.stringify({ provenance: provenanceFor(mode), results }, null, 2));
    const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
    writeFileSync(join(root, "real-" + mode, NORMALIZED_SIDECAR), JSON.stringify(sidecar));
  }
}

// {raw, gzipNormalized} ceilings equal to the saved runs under `root`.
export function ceilingsOf(root) {
  return Object.fromEntries(
    MODES.map((mode) => {
      const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
      return [
        mode,
        Object.fromEntries(
          Object.entries(sidecar.results).map(([fixture, result]) => [
            fixture,
            { raw: result.shipped.raw, gzipNormalized: result.shipped.gzipNormalized },
          ]),
        ),
      ];
    }),
  );
}

// A disposable git checkout for the artifact-less fingerprint-helper re-pin
// (#4198): the real helper committed at HEAD, both live contracts pinning this
// checkout's fingerprint, and `yaml` resolvable from the helper's directory.
// Edit `helperPath` afterwards to model the new helper.
export function makeHelperCheckout(roots) {
  const checkout = makeCheckout(roots);
  const { root } = checkout;
  const helper = SIZE_DIR + "/lock-fingerprint.mjs";
  copy(root, helper);
  const yamlDir = dirname(createRequire(import.meta.url).resolve("yaml/package.json"));
  mkdirSync(join(root, "node_modules"), { recursive: true });
  symlinkSync(yamlDir, join(root, "node_modules", "yaml"), "dir");
  const contracts = {};
  for (const [name, file] of [
    ["linux", "decision-linux-x64.json"],
    ["darwin", "decision.json"],
  ]) {
    const contract = JSON.parse(readFileSync(join(repoRoot, SIZE_DIR, file), "utf8"));
    contract.dependencyFingerprint = { ...checkout.dependencyFingerprint };
    // Last re-pinned by another reason, so the re-pin visibly moves `outcome`.
    contract.outcome = "release-version-stamp";
    write(root, SIZE_DIR + "/" + file, JSON.stringify(contract, null, 2) + "\n");
    contracts[name] = contract;
  }
  const git = (...args) =>
    execFileSync(
      "git",
      [
        "-c",
        "user.name=zfb-test",
        "-c",
        "user.email=zfb-test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        ...args,
      ],
      { cwd: root, stdio: "pipe" },
    );
  git("init", "-q");
  git("add", helper);
  git("commit", "-qm", "pinned helper");
  return { ...checkout, contracts, helperPath: join(root, helper), git };
}
