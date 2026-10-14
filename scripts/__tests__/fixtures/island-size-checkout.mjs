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
  realpathSync,
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
// `cargoLock` replaces the copied Cargo.lock and `pnpmLock` the copied
// pnpm-lock.yaml with the given text. The checkout carries its own copy of
// lock-fingerprint.mjs, which the rebaseline scripts load from the checkout.
export function makeCheckout(roots, { cargoLock, pnpmLock } = {}) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-checkout-"));
  roots.push(root);
  const lockText = pnpmLock ?? readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
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
  if (pnpmLock !== undefined) write(root, "pnpm-lock.yaml", pnpmLock);
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
  addHelper(root);
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
  const helper = HELPER_FILE;
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
  const git = gitIn(root);
  git("init", "-q");
  git("add", helper);
  git("commit", "-qm", "pinned helper");
  return { ...checkout, contracts, helperPath: join(root, helper), git };
}

const HELPER_FILE = SIZE_DIR + "/lock-fingerprint.mjs";

// Copies the real lock-fingerprint.mjs into the checkout with `yaml`
// resolvable from its directory, so its `--explain` runs against the checkout.
function addHelper(root) {
  const helper = HELPER_FILE;
  copy(root, helper);
  const yamlDir = dirname(createRequire(import.meta.url).resolve("yaml/package.json"));
  mkdirSync(join(root, "node_modules"), { recursive: true });
  symlinkSync(yamlDir, join(root, "node_modules", "yaml"), "dir");
  return helper;
}

// Test fixture — synthetic prior graph, not native evidence: a checkout whose
// pnpm-lock.yaml is `pnpmLock` (the new graph), with the helper and the live
// inventory file copied in. `priorFingerprint` is the contract-shape
// fingerprint the real lock has at this checkout, standing for the graph a
// contract pinned before the reviewed dependency change.
export function makeGraphCheckout(roots, { pnpmLock } = {}) {
  const checkout = makeCheckout(roots, { pnpmLock });
  copy(checkout.root, SIZE_DIR + "/lock-fingerprint-inventory.json");
  const prior = computeLockFingerprint({
    lockText: readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8"),
    currentVersion: checkout.version,
    repoRoot: checkout.root,
  });
  return {
    ...checkout,
    priorFingerprint: {
      policyVersion: prior.policyVersion,
      parser: prior.parser,
      helperSha256: prior.helperSha256,
      sha256: prior.sha256,
    },
    priorPnpmLockSha256: sha(readFileSync(join(repoRoot, "pnpm-lock.yaml"))),
  };
}

// Replaces exactly one occurrence of `from` in a helper source, so a fixture
// edit that no longer applies fails loudly instead of testing the live helper.
function replaceOnce(source, from, to) {
  const at = source.indexOf(from);
  if (at < 0 || source.indexOf(from, at + from.length) >= 0) {
    throw new Error(
      "test fixture: expected exactly one " + JSON.stringify(from) + " in the helper",
    );
  }
  return source.slice(0, at) + to + source.slice(at + from.length);
}

// Test fixture — synthetic prior policy, not native evidence: a git checkout
// whose COPY of lock-fingerprint.mjs is rewritten by `editHelper` (for example
// to `FINGERPRINT_POLICY_VERSION = 2`), with the live inventory and
// LOCK-FINGERPRINT-POLICY.md copied in. The live helper and its constants are
// untouched. `priorFingerprint` is what the live (policy 1) helper computes at
// this checkout, standing for the fingerprint the contracts pinned before the
// policy change; `dependencyFingerprint` and `inventory` are the edited
// helper's own `--explain` output. Commit with the returned `git` once the
// contracts are written.
export function makePolicyCheckout(roots, { editHelper } = {}) {
  const checkout = makeCheckout(roots);
  const { root } = checkout;
  copy(root, SIZE_DIR + "/lock-fingerprint-inventory.json");
  copy(root, SIZE_DIR + "/LOCK-FINGERPRINT-POLICY.md");
  const helperPath = join(root, HELPER_FILE);
  if (editHelper) writeFileSync(helperPath, editHelper(readFileSync(helperPath, "utf8")));
  const explain = JSON.parse(
    execFileSync(process.execPath, [realpathSync(helperPath), "--explain"]).toString("utf8"),
  );
  const { inventory, ...fingerprint } = explain;
  const git = gitIn(root);
  git("init", "-q");
  return {
    ...checkout,
    priorFingerprint: checkout.dependencyFingerprint,
    dependencyFingerprint: fingerprint,
    inventory,
    helperPath,
    git,
  };
}

// Test fixture — synthetic prior policy, not native evidence. These helper
// edits apply only to the COPY of lock-fingerprint.mjs inside a disposable
// checkout; the live helper stays at FINGERPRINT_POLICY_VERSION = 1, and no
// test here claims that a real policy transition occurred.
const POLICY_LINE = "export const FINGERPRINT_POLICY_VERSION = 1;";
export const policyHelper = {
  // A policy-2 helper: policyVersion is inside the canonical input, so sha256 moves too.
  v2: (source) => replaceOnce(source, POLICY_LINE, "export const FINGERPRINT_POLICY_VERSION = 2;"),
  // Policy 2 with a changed canonical form and parser identity string.
  v2Canonical: (source) =>
    replaceOnce(
      replaceOnce(
        replaceOnce(
          policyHelper.v2(source),
          'export const FINGERPRINT_PARSER = "yaml@2.9.0";',
          'export const FINGERPRINT_PARSER = "yaml@2.9.0+fixture";',
        ),
        "if (`yaml@${version}` !== FINGERPRINT_PARSER) {",
        'if (`yaml@${version}` !== "yaml@2.9.0") {',
      ),
      "    buildInputs,\n  });",
      "    buildInputs,\n    fixtureCanonicalForm: 2,\n  });",
    ),
  // Computes policy 2 but exports FINGERPRINT_POLICY_VERSION = 3.
  exports3: (source) =>
    replaceOnce(
      replaceOnce(
        replaceOnce(source, POLICY_LINE, "export const FINGERPRINT_POLICY_VERSION = 3;"),
        "    policyVersion: FINGERPRINT_POLICY_VERSION,\n    parser: FINGERPRINT_PARSER,\n    environment,",
        "    policyVersion: 2,\n    parser: FINGERPRINT_PARSER,\n    environment,",
      ),
      "  return {\n    policyVersion: FINGERPRINT_POLICY_VERSION,",
      "  return {\n    policyVersion: 2,",
    ),
};

// Test fixture: a git runner pinned to `root` with a fixed identity, no
// signing or hooks, and no inherited GIT_DIR-style redirection. Returns stdout.
function gitEnv() {
  const env = { ...process.env };
  for (const key of ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"]) {
    delete env[key];
  }
  return env;
}

export function gitIn(root) {
  const env = gitEnv();
  return (...args) =>
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
        "tag.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "init.defaultBranch=main",
        ...args,
      ],
      { cwd: root, stdio: "pipe", env },
    )
      .toString()
      .trim();
}

// Test fixture: a disposable git checkout whose single commit holds the OLD
// Cargo.lock and whatever `prepare` writes (a contract, an artifact); the NEW
// Cargo.lock is then written to the working tree only, as a fresh producer-lock
// move leaves it. `prepare` sees the identity a run at the new lock records.
export function makeGitCheckout(roots, { oldCargoLock, newCargoLock, prepare } = {}) {
  const checkout = makeCheckout(roots, { cargoLock: oldCargoLock });
  const atNew = {
    ...checkout,
    cargoLockSha256: newCargoLock === undefined ? checkout.cargoLockSha256 : sha(newCargoLock),
  };
  prepare?.(atNew);
  const git = gitIn(checkout.root);
  git("init", "-q");
  git("add", "-A");
  git("commit", "-qm", "test fixture: old Cargo.lock");
  const commit = git("rev-parse", "HEAD");
  if (newCargoLock !== undefined) write(checkout.root, "Cargo.lock", newCargoLock);
  return { ...atNew, git, commit };
}

// Test fixture: writes two dangling commit objects into the checkout whose
// SHAs share their first four hex digits, so that prefix is an ambiguous short
// commit name. Returns the shared prefix.
export function addAmbiguousCommitPrefix(root) {
  const git = gitIn(root);
  const tree = git("rev-parse", "HEAD^{tree}");
  const seen = new Map();
  for (let i = 0; ; i += 1) {
    const body =
      "tree " +
      tree +
      "\nauthor t <t@example.invalid> 0 +0000\ncommitter t <t@example.invalid> 0 +0000\n\ntest fixture " +
      i +
      "\n";
    const id = createHash("sha1")
      .update("commit " + Buffer.byteLength(body) + "\0" + body)
      .digest("hex");
    const prefix = id.slice(0, 4);
    const other = seen.get(prefix);
    if (other !== undefined) {
      for (const text of [other, body]) {
        execFileSync("git", ["hash-object", "-t", "commit", "-w", "--stdin"], {
          cwd: root,
          env: gitEnv(),
          input: text,
          stdio: ["pipe", "pipe", "pipe"],
        });
      }
      return prefix;
    }
    seen.set(prefix, body);
  }
}
