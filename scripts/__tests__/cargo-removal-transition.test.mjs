// cargo-removal (#4388). Test fixture — synthetic removal-only Cargo.lock
// transition, not native evidence: eleven-record locks whose external
// `deno_core`/`serde_v8` records leave and whose internal `zfb-js-host` /
// `zfb-render` records drop their `deno_core` edge (fixtures/island-size-
// cargo-removal.mjs). Pure proofs first, then end-to-end Linux CLI runs
// against one git-backed checkout whose tag `old-lock` holds the OLD
// Cargo.lock and whose HEAD holds the NEW lock, the pinned contract and the
// workspace manifests. The paired Darwin stage/apply lives in
// rebaseline-island-size-darwin.test.mjs. No live contract is read for
// anything but its shape, and none is written.
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";

import {
  CARGO_PRODUCER_LOCK_MOVEMENT,
  CARGO_REMOVAL_MOVEMENT,
  PAIRED_REASONS,
  REASONS,
  REMOVAL_ARGS,
  REPINNED_FROM_REASONS,
  SOURCE_BOUND_REASONS,
  applyStage,
  assertAllowedMovement,
  checkRemovalDeclaration,
  contractDiffKeys,
  main,
  parseArgs,
  proveCargoProducerLock,
  proveCargoRemoval,
  rewriteTestPins,
} from "../rebaseline-island-size-linux.mjs";
import { LOCAL_EVIDENCE_PROFILES, localEvidenceProfile } from "../island-size-local-evidence.mjs";
import {
  ceilingsOf,
  makeGitCheckout,
  withCurrentNormalizer,
  writeRuns,
} from "./fixtures/island-size-checkout.mjs";
import {
  DARWIN_TARGET,
  LINUX_TARGET,
  rewireDeclaration,
} from "./fixtures/island-size-cargo-rewire.mjs";
import {
  REMOVAL_MANIFESTS,
  editRecord,
  removalDeclaration,
  removalLocks,
  removalReceipt,
  removalRecordOf,
  removalSha,
  withoutRecord,
  writeRemovalInputs,
  writeRemovalManifests,
} from "./fixtures/island-size-cargo-removal.mjs";

// Synchronous git/helper end-to-end cases peak near 5.3 s on hosted runners (median up to 3.7 s); measured in #4364.
vi.setConfig({ testTimeout: 15_000 });

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const liveContract = JSON.parse(
  readFileSync(join(root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
// Verbatim released v4.5.3 Linux contract (main d70dd6d9): a genuine schemaVersion 3
// contract that predates every declaration-bound Cargo transition.
const preTransitionContract = JSON.parse(
  readFileSync(
    join(root, "scripts/__tests__/fixtures/island-size-linux-x64-v4.5.3-release.json"),
    "utf8",
  ),
);
const REPINNED_FROM_KEYS = [
  "schemaVersion",
  "outcome",
  "packageVersion",
  "baselineSourceSha",
  "baselineRunId",
  "contract",
];

const { oldText, newText, OLD, NEW } = removalLocks();
const SOURCE = { commit: "a".repeat(40), path: "Cargo.lock", sha256: OLD };
const DECLARATION_SOURCE = { path: "research/v3-island-size/removal.json", sha256: "1".repeat(64) };
const manifests = (edit = (_path, text) => text) => {
  const files = Object.fromEntries(
    Object.entries(REMOVAL_MANIFESTS).map(([path, text]) => [path, edit(path, text)]),
  );
  return (path) => {
    if (!Object.hasOwn(files, path)) throw new Error("ENOENT " + path);
    return Buffer.from(files[path]);
  };
};
// Proof inputs for the fixture transition, or for a variant `newLock` the
// checkout and artifact both carry (the declaration and the new receipt are
// re-bound to its digest so only the named problem remains). `declare` edits
// the declaration, `oldClosure` / `newClosure` the receipts.
function inputs({
  newLock = newText,
  declare = () => {},
  oldClosure,
  newClosure,
  target = LINUX_TARGET,
  readManifest = manifests(),
} = {}) {
  const NEW_SHA = removalSha(newLock);
  return {
    contractSha: OLD,
    artifactSha: NEW_SHA,
    expectedOldSha: OLD,
    expectedNewSha: NEW_SHA,
    oldText,
    newText: newLock,
    oldLockSource: SOURCE,
    newLockSource: { sha256: NEW_SHA },
    declaration: removalDeclaration(declare, { oldText, newText: newLock }),
    declarationSource: DECLARATION_SOURCE,
    closures: {
      old: { receipt: removalReceipt("old", target, oldClosure), sha256: "2".repeat(64) },
      new: {
        receipt: removalReceipt("new", target, newClosure, newLock),
        sha256: "3".repeat(64),
      },
    },
    target: LINUX_TARGET,
    readManifest,
  };
}
const prove = (options, edit = (i) => i) => proveCargoRemoval(edit(inputs(options)));
const row = (receipt, name) => receipt.rows.find((r) => r.name === name);

describe("cargo-removal (#4388) proof", () => {
  it("is a paired, source-bound reason with the exact Cargo movement and a local profile", () => {
    expect(REASONS["cargo-removal"]).toEqual({
      outcome: "cargo-removal-transition",
      heading: "cargo-removal transition",
      requires: 3,
      accepts: [3],
    });
    expect(PAIRED_REASONS).toContain("cargo-removal");
    expect(SOURCE_BOUND_REASONS).toContain("cargo-removal");
    expect(REPINNED_FROM_REASONS).toContain("cargo-removal");
    expect(CARGO_REMOVAL_MOVEMENT).toEqual(CARGO_PRODUCER_LOCK_MOVEMENT);
    expect(REMOVAL_ARGS).toEqual([
      "removal-declaration",
      "old-closure",
      "new-closure",
      "new-cargo-ref",
    ]);
    expect(LOCAL_EVIDENCE_PROFILES["cargo-removal"]).toMatchObject({
      transitionKey: "cargoRemoval",
      requiresReviewedTransition: false,
    });
    expect(localEvidenceProfile("cargo-removal")).toBe(LOCAL_EVIDENCE_PROFILES["cargo-removal"]);
  });

  it("proves the declared removal and records exact bytes, edges, sources and closure rows", () => {
    expect(prove()).toEqual({
      oldSha: OLD,
      newSha: NEW,
      oldLockSource: SOURCE,
      newLockSource: { sha256: NEW },
      declaration: DECLARATION_SOURCE,
      producer: "zfb",
      manifestsAtCommit: null,
      added: [],
      removed: [
        {
          id: "deno_core@0.399.0",
          sha256: removalSha(removalRecordOf(oldText, "deno_core@0.399.0").body),
          source: "registry+https://github.com/rust-lang/crates.io-index",
        },
        {
          id: "serde_v8@0.250.0",
          sha256: removalSha(removalRecordOf(oldText, "serde_v8@0.250.0").body),
          source: "registry+https://github.com/rust-lang/crates.io-index",
        },
      ],
      changed: ["zfb-js-host", "zfb-render"].map((name) => ({
        id: name + "@0.0.0",
        manifest: "crates/" + name + "/Cargo.toml",
        manifestSha256: removalSha(REMOVAL_MANIFESTS["crates/" + name + "/Cargo.toml"]),
        oldSha256: removalSha(removalRecordOf(oldText, name + "@0.0.0").body),
        newSha256: removalSha(removalRecordOf(newText, name + "@0.0.0").body),
        removedDependencies: ["deno_core"],
      })),
      closure: {
        target: LINUX_TARGET,
        oldSha256: "2".repeat(64),
        newSha256: "3".repeat(64),
        rows: { old: 7, new: 7, unchanged: 7 },
        removed: [],
      },
    });
    // Raw bytes are hashed before decoding: Buffers prove like their text.
    expect(
      prove({}, (i) => ({ ...i, oldText: Buffer.from(oldText), newText: Buffer.from(newText) }))
        .newSha,
    ).toBe(NEW);
  });

  it("admits an explicitly declared internal row leaving the producer closure", () => {
    const transition = prove({
      newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "zfb-js-host")),
      declare: (d) => (d.closure.removed = ["zfb-js-host@0.0.0"]),
    });
    expect(transition.closure).toMatchObject({
      rows: { old: 7, new: 6, unchanged: 6 },
      removed: ["zfb-js-host@0.0.0"],
    });
  });

  it("admits a removed internal record that no producer edge reaches", () => {
    const oldLock = oldText.replace(
      '[[package]]\nname = "zudo-wind"',
      '[[package]]\nname = "zfb-v8-shim"\nversion = "0.0.0"\ndependencies = [\n "deno_core",\n]\n\n[[package]]\nname = "zudo-wind"',
    );
    const transition = proveCargoRemoval({ ...inputs(), ...rebindOld(oldLock) });
    expect(transition.removed.map((r) => [r.id, r.source])).toContainEqual([
      "zfb-v8-shim@0.0.0",
      null,
    ]);
  });

  // An alternative old lock: the contract, the blob and the declaration follow it.
  function rebindOld(oldLock) {
    const OLD_SHA = removalSha(oldLock);
    const base = inputs();
    const declaration = removalDeclaration(
      (d) =>
        d.removed.push({
          id: "zfb-v8-shim@0.0.0",
          sha256: removalSha(removalRecordOf(oldLock, "zfb-v8-shim@0.0.0").body),
        }),
      { oldText: oldLock, newText },
    );
    return {
      contractSha: OLD_SHA,
      expectedOldSha: OLD_SHA,
      oldText: oldLock,
      oldLockSource: { ...SOURCE, sha256: OLD_SHA },
      declaration,
      closures: {
        ...base.closures,
        old: {
          receipt: removalReceipt("old", LINUX_TARGET, undefined, oldLock),
          sha256: "2".repeat(64),
        },
      },
    };
  }

  it("normalizes a declaration and refuses a malformed one", () => {
    const normalized = checkRemovalDeclaration(removalDeclaration());
    expect(normalized.closure).toEqual({ removed: [] });
    expect(normalized.removed.map((r) => r.id)).toEqual(["deno_core@0.399.0", "serde_v8@0.250.0"]);
    for (const [name, edit, message] of [
      ["a wrong schema", (d) => (d.schemaVersion = 2), /schemaVersion must be 1/],
      ["an added key", (d) => (d.added = []), /must carry exactly schemaVersion, producer/],
      ["no closure", (d) => delete d.closure, /must carry exactly/],
      ["another producer", (d) => (d.producer = "zudo-wind"), /producer must be zfb/],
      ["a short hash", (d) => (d.oldCargoLockSha256 = "abc"), /oldCargoLockSha256 must be/],
      ["equal hashes", (d) => (d.newCargoLockSha256 = d.oldCargoLockSha256), /no Cargo\.lock move/],
      ["no removed record", (d) => (d.removed = []), /declares no removed record/],
      ["a bad removed id", (d) => (d.removed[0].id = "deno_core"), /removed entries need exactly/],
      [
        "an extra removed key",
        (d) => (d.removed[0].manifest = "crates/x/Cargo.toml"),
        /removed entries need exactly/,
      ],
      [
        "added edges on a changed record",
        (d) => (d.changed[0].addedDependencies = []),
        /changed entries need exactly id, manifest, oldSha256, newSha256, removedDependencies/,
      ],
      ["a bad changed id", (d) => (d.changed[0].id = "zfb-render"), /id of the form name@version/],
      [
        "a .. manifest",
        (d) => (d.changed[0].manifest = "../x/Cargo.toml"),
        /must not contain a \.\. segment/,
      ],
      [
        "a non-manifest path",
        (d) => (d.changed[0].manifest = "crates/x/lib.rs"),
        /must be a Cargo\.toml path/,
      ],
      [
        "identical changed hashes",
        (d) => (d.changed[0].newSha256 = d.changed[0].oldSha256),
        /identical old and new bytes/,
      ],
      [
        "no removed edge",
        (d) => (d.changed[0].removedDependencies = []),
        /needs a nonempty removedDependencies array/,
      ],
      [
        "a repeated edge",
        (d) => (d.changed[0].removedDependencies = ["deno_core", "deno_core"]),
        /repeats deno_core/,
      ],
      [
        "a malformed edge",
        (d) => (d.changed[0].removedDependencies = ["a b c d"]),
        /malformed removedDependencies entry/,
      ],
      ["a repeated id", (d) => d.changed.push({ ...d.changed[0] }), /repeats zfb-js-host@0\.0\.0/],
      ["a bad closure", (d) => (d.closure = { removed: ["x"] }), /closure must carry exactly/],
      [
        "a closure with an extra key",
        (d) => (d.closure = { removed: [], changed: [] }),
        /closure must carry exactly/,
      ],
    ]) {
      expect(() => checkRemovalDeclaration(removalDeclaration(edit)), name).toThrow(message);
    }
    expect(() => checkRemovalDeclaration("x")).toThrow(/must be a JSON object/);
    // A rewire declaration is not a removal declaration.
    expect(() => checkRemovalDeclaration(rewireDeclaration())).toThrow(/must carry exactly/);
  });

  const add = (text) =>
    text.replace(
      '[[package]]\nname = "sha2"',
      '[[package]]\nname = "serde"\nversion = "1.0.219"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "' +
        "4".repeat(64) +
        '"\n\n[[package]]\nname = "sha2"',
    );
  // The spelling rewrite Cargo itself writes: removing `itoa 0.4.8` leaves one
  // itoa, so the external serde_json record's "itoa 1.0.15" becomes "itoa".
  const spellingLock = editRecord(
    editRecord(withoutRecord(newText, "itoa@0.4.8"), "serde_json@1.0.140", (body) =>
      body.replace('"itoa 1.0.15"', '"itoa"'),
    ),
    "zudo-wind@0.0.0",
    (body) => body.replace(' "itoa 0.4.8",\n', ""),
  );
  it.each([
    {
      name: "an undeclared removal",
      options: { declare: (d) => d.removed.pop() },
      message: /undeclared removed records serde_v8@0\.250\.0/,
    },
    {
      name: "an addition",
      options: { newLock: add(newText) },
      message: /added records serde@1\.0\.219 \(cargo-removal admits no addition\)/,
    },
    {
      name: "a version bump (a removal plus an addition)",
      options: {
        newLock: newText.replace(
          'name = "anyhow"\nversion = "1.0.100"',
          'name = "anyhow"\nversion = "1.0.101"',
        ),
        newClosure: (r) => (row(r, "anyhow").version = "1.0.101"),
      },
      message: /added records anyhow@1\.0\.101.*undeclared removed records anyhow@1\.0\.100/,
    },
    {
      name: "an external record change",
      options: {
        newLock: editRecord(newText, "anyhow@1.0.100", (body) =>
          body.replace(/checksum = "[0-9a-f]+"/, 'checksum = "' + "5".repeat(64) + '"'),
        ),
      },
      message:
        /changed external records anyhow@1\.0\.100 \(an external record may not change, not even its dependency spelling\)/,
    },
    {
      name: "a dependency-spelling rewrite of an external record",
      options: {
        newLock: spellingLock,
        declare: (d, { oldText: old, newText: next, sha }) => {
          d.removed.splice(1, 0, {
            id: "itoa@0.4.8",
            sha256: sha(removalRecordOf(old, "itoa@0.4.8").body),
          });
          d.changed.push({
            id: "zudo-wind@0.0.0",
            manifest: "crates/zudo-wind/Cargo.toml",
            oldSha256: sha(removalRecordOf(old, "zudo-wind@0.0.0").body),
            newSha256: sha(removalRecordOf(next, "zudo-wind@0.0.0").body),
            removedDependencies: ["itoa 0.4.8"],
          });
        },
        newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "itoa")),
      },
      message: /changed external records serde_json@1\.0\.140 \(an external record may not change/,
    },
    {
      name: "a declared change of an external record",
      options: {
        newLock: editRecord(newText, "serde_json@1.0.140", (body) =>
          body.replace('"itoa 1.0.15"', '"itoa"'),
        ),
        declare: (d, { oldText: old, newText: next, sha }) =>
          d.changed.push({
            id: "serde_json@1.0.140",
            manifest: "crates/zudo-wind/Cargo.toml",
            oldSha256: sha(removalRecordOf(old, "serde_json@1.0.140").body),
            newSha256: sha(removalRecordOf(next, "serde_json@1.0.140").body),
            removedDependencies: ["itoa 1.0.15"],
          }),
      },
      message:
        /declared changed serde_json@1\.0\.140 has a source or checksum; not an internal record.*adds edges \["itoa"\]/,
    },
    {
      name: "an internal spelling rewrite (an added edge)",
      options: {
        newLock: editRecord(newText, "zfb-render@0.0.0", (body) =>
          body.replace('"sha2"', '"sha2 0.10.9"'),
        ),
        declare: (d) => (d.changed[1].removedDependencies = ["deno_core", "sha2"]),
      },
      message:
        /declared changed zfb-render@0\.0\.0 adds edges \["sha2 0\.10\.9"\] \(cargo-removal admits only removed edges/,
    },
    {
      name: "an undeclared internal edge removal",
      options: {
        newLock: editRecord(newText, "zudo-wind@0.0.0", (body) =>
          body.replace(' "serde_json",\n', ""),
        ),
      },
      message: /undeclared changed records zudo-wind@0\.0\.0/,
    },
    {
      name: "removed edges that differ from the declaration",
      options: { declare: (d) => (d.changed[1].removedDependencies = ["deno_core", "sha2"]) },
      message:
        /declared changed zfb-render@0\.0\.0 removed edges \["deno_core"\] differ from removedDependencies/,
    },
    {
      name: "reordered remaining edges",
      options: {
        newLock: editRecord(newText, "zfb-render@0.0.0", (body) =>
          body.replace(' "anyhow",\n "sha2",', ' "sha2",\n "anyhow",'),
        ),
      },
      message: /declared changed zfb-render@0\.0\.0 reorders its remaining dependency edges/,
    },
    {
      name: "a moved field other than dependencies",
      options: {
        newLock: editRecord(newText, "zfb-render@0.0.0", (body) =>
          body.replace("dependencies = [", 'source = "path+x"\ndependencies = ['),
        ),
      },
      message:
        /declared changed zfb-render@0\.0\.0 moved a field other than dependencies.*not an internal record/,
    },
    {
      name: "a wrong removed-record hash",
      options: { declare: (d) => (d.removed[0].sha256 = "6".repeat(64)) },
      message: /declared removed deno_core@0\.399\.0 bytes hash to [0-9a-f]{64}, not its sha256/,
    },
    {
      name: "a declared removal that is still present",
      options: {
        declare: (d, { oldText: old, sha }) =>
          d.removed.push({
            id: "sha2@0.10.9",
            sha256: sha(removalRecordOf(old, "sha2@0.10.9").body),
          }),
      },
      message: /declared removed sha2@0\.10\.9 is still in the new Cargo\.lock/,
    },
    {
      name: "a declared removal the old lock never had",
      options: {
        declare: (d) => d.removed.push({ id: "deno_ops@0.1.0", sha256: "7".repeat(64) }),
      },
      message: /declared removed deno_ops@0\.1\.0 is not in the old Cargo\.lock/,
    },
    {
      name: "a wrong changed-record hash",
      options: { declare: (d) => (d.changed[0].newSha256 = "6".repeat(64)) },
      message: /declared changed zfb-js-host@0\.0\.0 new bytes hash to/,
    },
    {
      name: "an edge left pointing at a removed record",
      options: {
        newLock: editRecord(newText, "zfb-render@0.0.0", (body) =>
          body.replace(' "anyhow",', ' "anyhow",\n "deno_core",'),
        ),
        declare: (d) => d.changed.pop(),
      },
      message:
        /zfb-render@0\.0\.0: dependency entry "deno_core" names no record in the new Cargo\.lock/,
    },
    {
      name: "a changed header",
      options: { newLock: newText.replace("version = 4\n", "version = 3\n") },
      message: /the header differs/,
    },
    {
      name: "a manifest without publish = false",
      options: {
        readManifest: manifests((path, text) =>
          path.endsWith("zfb-render/Cargo.toml") ? text.replace("publish = false\n", "") : text,
        ),
      },
      message: /zfb-render@0\.0\.0: crates\/zfb-render\/Cargo\.toml does not set publish = false/,
    },
    {
      name: "an external closure row leaving",
      options: { newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "sha2")) },
      message:
        /external rows left the producer closure: sha2@0\.10\.9 \(registry\+https:\/\/github\.com\/rust-lang\/crates\.io-index\)/,
    },
    {
      name: "an undeclared internal closure row leaving",
      options: { newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "zfb-render")) },
      message:
        /closure rows removed \["zfb-render@0\.0\.0"\] differ from the declaration's closure\.removed/,
    },
    {
      name: "a declared closure row that stays",
      options: { declare: (d) => (d.closure.removed = ["zfb-render@0.0.0"]) },
      message: /closure rows removed \[\] differ from the declaration's closure\.removed/,
    },
    {
      name: "a closure row joining",
      options: {
        newClosure: (r) =>
          r.rows.push({
            name: "serde_json",
            version: "1.0.140",
            source: "registry+https://github.com/rust-lang/crates.io-index",
            features: ["default"],
          }),
      },
      message:
        /rows joined the producer closure: serde_json@1\.0\.140 \(registry\+https:\/\/github\.com\/rust-lang\/crates\.io-index\)/,
    },
    {
      name: "an external feature move",
      options: { newClosure: (r) => (row(r, "anyhow").features = ["std"]) },
      message:
        /feature movement in the producer closure: anyhow@1\.0\.100 .*\["default","std"\] -> \["std"\]/,
    },
    {
      name: "an internal feature move",
      options: { newClosure: (r) => (row(r, "zfb-render").features = ["default", "embed_v8"]) },
      message: /feature movement in the producer closure: zfb-render@0\.0\.0/,
    },
    {
      name: "a receipt without the default-feature command",
      options: { newClosure: (r) => r.command.push("--all-features") },
      message: /new closure receipt must attest the default-feature metadata command/,
    },
    {
      name: "a receipt whose producer lacks its default feature",
      options: { oldClosure: (r) => (row(r, "zfb").features = []) },
      message: /old closure must enable the producer's default feature/,
    },
    {
      name: "a Darwin receipt on Linux",
      options: { target: DARWIN_TARGET },
      message:
        /old closure receipt was recorded for target aarch64-apple-darwin, not x86_64-unknown-linux-gnu/,
    },
    {
      name: "a receipt from another lock",
      options: { newClosure: (r) => (r.cargoLockSha256 = "7".repeat(64)) },
      message: /new closure receipt was recorded at Cargo\.lock 7{64}/,
    },
  ])("refuses $name", ({ options, message }) => {
    expect(() => prove(options)).toThrow(message);
    expect(() => prove(options)).toThrow(/^--reason cargo-removal: /);
  });

  it.each([
    [
      "a wrong old hash",
      (i) => ({ ...i, expectedOldSha: "1".repeat(64) }),
      /the contract pins Cargo\.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
    ],
    [
      "a wrong new hash",
      (i) => ({ ...i, expectedNewSha: "2".repeat(64) }),
      /the checkout's Cargo\.lock hashes to [0-9a-f]{64}, not --expected-new-cargo-sha/,
    ],
    [
      "an artifact measured at another lock",
      (i) => ({ ...i, artifactSha: "3".repeat(64) }),
      /the artifact measured Cargo\.lock 3{64}, not --expected-new-cargo-sha/,
    ],
    [
      "a filesystem old-lock source",
      (i) => ({ ...i, oldLockSource: "/tmp/old.lock" }),
      /must be its verified git blob \{commit, path, sha256\}/,
    ],
    [
      "a declaration bound to another old lock",
      (i) => ({ ...i, declaration: { ...i.declaration, oldCargoLockSha256: "5".repeat(64) } }),
      /the declaration's oldCargoLockSha256 is not --expected-old-cargo-sha/,
    ],
    [
      "a declaration without a recorded digest",
      (i) => ({ ...i, declarationSource: { path: "x.json" } }),
      /the declaration source must record its path and sha256/,
    ],
  ])("refuses %s before reading the locks", (_name, edit, message) => {
    expect(() => prove({}, edit)).toThrow(message);
  });

  it("points cargo-producer-lock at this reason for a removed record", () => {
    expect(() =>
      proveCargoProducerLock({
        contractSha: OLD,
        artifactSha: NEW,
        expectedOldSha: OLD,
        expectedNewSha: NEW,
        oldText,
        newText,
        added: [],
        oldLockSource: SOURCE,
      }),
    ).toThrow(
      /removed records deno_core@0\.399\.0, serde_v8@0\.250\.0 \(a reviewed removal-only transition needs --reason cargo-removal\)/,
    );
  });
});

// End-to-end Linux CLI runs against one git-backed checkout.
describe("cargo-removal (#4388) end to end", () => {
  const gitRoots = [];
  const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const OUTPUTS = [CONTRACT, TEST, README];
  const SOURCE_SHA = "b".repeat(40);
  const HEAD_SHA = "c".repeat(40);
  let repo;
  let artifact;
  let pinned;
  let removalInputs;
  let saved;
  let newLockCommit;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
  const tempDir = (prefix) => {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    gitRoots.push(dir);
    return dir;
  };
  const linuxArtifact = (edit, cargoLockSha256 = NEW) => {
    const dir = tempDir("zfb-removal-artifact-");
    writeRuns(dir, Object.keys(liveContract.ceilings.workspace), (mode) => {
      const provenance = {
        sourceSha: SOURCE_SHA,
        mode,
        platform: { os: "linux", arch: "x64" },
        fixtureVersion: liveContract.fixtureVersion,
        fixtureSha256: repo.fixtureSha256,
        runnerSha256: repo.runnerSha256,
        packageVersion: repo.version,
        runtimePackageVersion: repo.version,
        honoPackageVersion: liveContract.toolchain.honoVersion,
        esbuildVersion: liveContract.toolchain.esbuildVersion,
        nodeVersion: liveContract.toolchain.nodeVersion,
        zlibVersion: liveContract.toolchain.zlibVersion,
        compression: liveContract.compression,
        repeats: 2,
        pnpmLockSha256: repo.pnpmLockSha256,
        dependencyFingerprint: {
          policyVersion: repo.dependencyFingerprint.policyVersion,
          sha256: repo.dependencyFingerprint.sha256,
        },
        cargoLockSha256,
      };
      edit?.(provenance, mode);
      return provenance;
    });
    return dir;
  };

  beforeAll(() => {
    repo = makeGitCheckout(gitRoots, {
      oldCargoLock: oldText,
      newCargoLock: newText,
      prepare: (checkout) => writeRemovalManifests(checkout.root),
    });
    repo.git("tag", "old-lock");
    artifact = linuxArtifact();
    pinned = withCurrentNormalizer(structuredClone(preTransitionContract));
    pinned.ceilings = ceilingsOf(artifact);
    pinned.fixtureSha256 = { ...repo.fixtureSha256 };
    pinned.runnerSha256 = { ...repo.runnerSha256 };
    pinned.toolchain.packageVersion = repo.version;
    pinned.toolchain.pnpmLockSha256 = repo.pnpmLockSha256;
    pinned.toolchain.cargoLockSha256 = OLD;
    pinned.dependencyFingerprint = { ...repo.dependencyFingerprint };
    writeFileSync(at(CONTRACT), contractText(pinned));
    removalInputs = writeRemovalInputs(at("research/v3-island-size/removal"), LINUX_TARGET);
    // HEAD now stores the new lock, the pinned contract and the inputs.
    repo.git("add", "-A");
    repo.git("commit", "-qm", "test fixture: new Cargo.lock, pinned contract, removal inputs");
    newLockCommit = repo.git("rev-parse", "HEAD");
    saved = Object.fromEntries([...OUTPUTS, "Cargo.lock"].map((file) => [file, read(file)]));
  });
  afterEach(() => {
    repo.git("reset", "--mixed", newLockCommit);
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    writeRemovalManifests(repo.root);
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of gitRoots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const argv = (extra = {}) =>
    Object.entries({
      artifact,
      "run-id": "91",
      "job-id": "92",
      "head-sha": HEAD_SHA,
      "source-sha": SOURCE_SHA,
      reason: "cargo-removal",
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-ref": "old-lock",
      ...removalInputs,
      stage: join(tempDir("zfb-removal-stage-"), "stage"),
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    return vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const unchanged = (before) =>
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
  const live = () => OUTPUTS.map((file) => saved[file]);

  it("stages the removal with exact-key movement, cited sources and a bound run identity", async () => {
    const notices = quiet();
    const stage = join(tempDir("zfb-removal-stage-"), "stage");
    await main(argv({ stage }), repo.root);
    unchanged(live());
    expect(notices).not.toHaveBeenCalled();
    const next = JSON.parse(readFileSync(join(stage, CONTRACT), "utf8"));
    expect(
      contractDiffKeys(pinned, next).filter((key) => !key.startsWith("baselineEvidence.")),
    ).toEqual(["baselineSourceSha", "outcome", "platformStatus", "toolchain.cargoLockSha256"]);
    expect(() =>
      assertAllowedMovement("cargo-removal", CARGO_REMOVAL_MOVEMENT, pinned, next),
    ).not.toThrow();
    expect(next.toolchain).toEqual({ ...pinned.toolchain, cargoLockSha256: NEW });
    expect(next.ceilings).toEqual(pinned.ceilings);
    expect(next.allowance).toEqual(pinned.allowance);
    expect(next.fixtureSha256).toEqual(pinned.fixtureSha256);
    expect(next.dependencyFingerprint).toEqual(pinned.dependencyFingerprint);
    expect(next.outcome).toBe("cargo-removal-transition");
    expect(next.baselineSourceSha).toBe(HEAD_SHA);
    expect(Object.keys(next.baselineEvidence)).toEqual([
      "event",
      "runId",
      "jobId",
      "sourceSha",
      "repinnedFrom",
      "cargoRemoval",
      "actualGzip",
      "note",
    ]);
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    const removal = next.baselineEvidence.cargoRemoval;
    expect(removal.oldLockSource).toEqual({ commit: repo.commit, path: "Cargo.lock", sha256: OLD });
    expect(removal.newLockSource).toEqual({
      sha256: NEW,
      commit: newLockCommit,
      path: "Cargo.lock",
    });
    expect(removal.declaration).toEqual({
      path: "research/v3-island-size/removal/declaration.json",
      sha256: removalSha(readFileSync(removalInputs["removal-declaration"])),
    });
    expect(removal.manifestsAtCommit).toBe(newLockCommit);
    expect(removal.removed.map((r) => r.id)).toEqual(["deno_core@0.399.0", "serde_v8@0.250.0"]);
    expect(removal.changed.map((r) => [r.id, r.removedDependencies])).toEqual([
      ["zfb-js-host@0.0.0", ["deno_core"]],
      ["zfb-render@0.0.0", ["deno_core"]],
    ]);
    expect(removal.closure).toEqual({
      target: LINUX_TARGET,
      oldSha256: removalSha(readFileSync(removalInputs["old-closure"])),
      newSha256: removalSha(readFileSync(removalInputs["new-closure"])),
      rows: { old: 7, new: 7, unchanged: 7 },
      removed: [],
    });
    expect(next.platformStatus).toContain("after a reviewed removal-only Cargo.lock transition");
    expect(next.baselineEvidence.note.startsWith("Cargo removal transition run at v")).toBe(true);
    const readme = readFileSync(join(stage, README), "utf8");
    expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
    expect(readme).toContain("## v" + repo.version + " cargo-removal transition");
    expect(readme).toContain("`zfb-render@0.0.0` drops `deno_core`");
    expect(readme).toContain("Darwin arm64 contract moves in the same paired change");
    expect(readFileSync(join(stage, TEST), "utf8")).toBe(
      rewriteTestPins(saved[TEST].toString("utf8"), next),
    );
    const manifest = JSON.parse(readFileSync(join(stage, "stage-manifest.json"), "utf8"));
    expect(manifest.runs).toHaveLength(1);
    expect(manifest.runs[0]).toMatchObject({
      reason: "cargo-removal",
      platform: "linux-x64",
      sourceSha: SOURCE_SHA,
      headSha: HEAD_SHA,
      removal: {
        declarationSha256: removal.declaration.sha256,
        oldLockSha256: OLD,
        newLockSha256: NEW,
      },
    });
    expect(manifest.runs[0]).not.toHaveProperty("rewire");
    // Linux alone cannot land: the paired Darwin run is missing.
    expect(() => applyStage(stage, repo.root, () => {})).toThrow(
      /--reason cargo-removal is not staged for darwin-arm64; stage both platforms into the same directory first \(nothing was applied\)/,
    );
    unchanged(live());
  });

  it("refuses dirty manifests when the new lock automatically cites HEAD", async () => {
    quiet();
    const before = OUTPUTS.map(read);
    const stage = join(tempDir("zfb-removal-stage-"), "stage");
    writeRemovalManifests(repo.root, (path, text) =>
      path.endsWith("zfb-render/Cargo.toml") ? text + "# uncommitted manifest edit\n" : text,
    );
    await expect(main(argv({ stage }), repo.root)).rejects.toThrow(
      /--reason cargo-removal: crates\/zfb-render\/Cargo\.toml in the working tree differs from [0-9a-f]{40}:crates\/zfb-render\/Cargo\.toml; refusing/,
    );
    expect(existsSync(stage)).toBe(false);
    unchanged(before);
  });

  // A variant new lock in the working tree (HEAD keeps the fixture lock, so it
  // is cited by digest only) with the artifact, declaration and new receipt
  // re-bound to it, so only the named problem remains.
  const variant = (lock, { declare, newClosure } = {}) => {
    writeFileSync(at("Cargo.lock"), lock);
    const LOCK = removalSha(lock);
    const dir = tempDir("zfb-removal-inputs-");
    const values = writeRemovalInputs(dir, LINUX_TARGET, {
      declaration: (d) => Object.assign(d, removalDeclaration(declare, { oldText, newText: lock })),
      newClosure: (r) => {
        r.cargoLockSha256 = LOCK;
        newClosure?.(r);
      },
    });
    return { values, LOCK, artifact: linuxArtifact(undefined, LOCK) };
  };
  it.each([
    {
      name: "an undeclared removal",
      inputs: { declaration: (d) => d.removed.pop() },
      message: /undeclared removed records serde_v8@0\.250\.0/,
    },
    {
      name: "a closure row leaving",
      inputs: { newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "sha2")) },
      message: /external rows left the producer closure: sha2@0\.10\.9/,
    },
    {
      name: "a closure row joining",
      inputs: {
        newClosure: (r) =>
          r.rows.push({
            name: "itoa",
            version: "1.0.15",
            source: "registry+https://github.com/rust-lang/crates.io-index",
            features: [],
          }),
      },
      message: /rows joined the producer closure: itoa@1\.0\.15/,
    },
    {
      name: "a feature move",
      inputs: { newClosure: (r) => (row(r, "sha2").features = ["default"]) },
      message: /feature movement in the producer closure: sha2@0\.10\.9/,
    },
    {
      name: "a declaration for another producer",
      inputs: { declaration: (d) => (d.producer = "zudo-wind") },
      message: /producer must be zfb, the measured CLI package, not "zudo-wind"/,
    },
  ])("refuses $name end to end and writes nothing", async ({ inputs: edits, message }) => {
    quiet();
    const values = writeRemovalInputs(tempDir("zfb-removal-inputs-"), LINUX_TARGET, edits);
    const before = OUTPUTS.map(read);
    const stage = join(tempDir("zfb-removal-stage-"), "stage");
    await expect(main(argv({ stage, ...values }), repo.root)).rejects.toThrow(message);
    unchanged(before);
    expect(existsSync(stage)).toBe(false);
  });

  it.each([
    {
      name: "an addition",
      lock: newText.replace(
        '[[package]]\nname = "sha2"',
        '[[package]]\nname = "serde"\nversion = "1.0.219"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "' +
          "4".repeat(64) +
          '"\n\n[[package]]\nname = "sha2"',
      ),
      message: /added records serde@1\.0\.219 \(cargo-removal admits no addition\)/,
    },
    {
      name: "an external record change",
      lock: editRecord(newText, "sha2@0.10.9", (body) =>
        body.replace(/checksum = "[0-9a-f]+"/, 'checksum = "' + "5".repeat(64) + '"'),
      ),
      message: /changed external records sha2@0\.10\.9/,
    },
    {
      name: "a dependency-spelling rewrite",
      lock: editRecord(
        editRecord(withoutRecord(newText, "itoa@0.4.8"), "serde_json@1.0.140", (body) =>
          body.replace('"itoa 1.0.15"', '"itoa"'),
        ),
        "zudo-wind@0.0.0",
        (body) => body.replace(' "itoa 0.4.8",\n', ""),
      ),
      declare: (d, { oldText: old, sha, changed }) => {
        d.removed.splice(1, 0, {
          id: "itoa@0.4.8",
          sha256: sha(removalRecordOf(old, "itoa@0.4.8").body),
        });
        d.changed.push(changed("zudo-wind@0.0.0", "crates/zudo-wind/Cargo.toml", ["itoa 0.4.8"]));
      },
      newClosure: (r) => (r.rows = r.rows.filter((x) => x.name !== "itoa")),
      message: /changed external records serde_json@1\.0\.140/,
    },
  ])(
    "refuses $name in the checkout's Cargo.lock end to end and writes nothing",
    async ({ lock, declare, newClosure, message }) => {
      quiet();
      const { values, LOCK, artifact: measured } = variant(lock, { declare, newClosure });
      const before = OUTPUTS.map(read);
      const stage = join(tempDir("zfb-removal-stage-"), "stage");
      await expect(
        main(
          argv({ stage, artifact: measured, "expected-new-cargo-sha": LOCK, ...values }),
          repo.root,
        ),
      ).rejects.toThrow(message);
      unchanged(before);
      expect(existsSync(stage)).toBe(false);
    },
  );

  const identity = (name, edit) => ({
    name: "a changed " + name,
    contract: edit,
    message: new RegExp(
      "--reason cargo-removal: allows only Cargo\\.lock to move, but " +
        name.replace(".", "\\.") +
        " differ from the contract",
    ),
  });
  it.each([
    {
      name: "raw +1",
      contract: (c) => (c.ceilings.workspace.model.raw -= 1),
      message: /raw totals changed \(workspace\/model\); this is not a cargo-removal re-pin/,
    },
    {
      name: "gzipNormalized -1",
      contract: (c) => (c.ceilings.packed["json-api"].gzipNormalized += 1),
      message: /gzipNormalized totals changed \(packed\/json-api -1\)/,
    },
    {
      name: "a changed fixture set",
      contract: (c) => (c.ceilings.workspace.extra = { raw: 1, gzipNormalized: 1 }),
      message: /fixture set differs from the contract \(workspace\/extra\)/,
    },
    {
      name: "a moved dependency fingerprint",
      contract: (c) => (c.dependencyFingerprint.sha256 = "8".repeat(64)),
      message: /--reason cargo-removal cannot move the dependency fingerprint \(sha256 differ/,
    },
    identity("packageVersion", (c) => (c.toolchain.packageVersion = "0.0.1")),
    identity("pnpmLockSha256", (c) => (c.toolchain.pnpmLockSha256 = "f".repeat(64))),
    identity("normalizerSha256", (c) => (c.normalizerSha256 = "e".repeat(64))),
    identity("fixtureSha256", (c) => (c.fixtureSha256.model = "0".repeat(64))),
    identity("runnerSha256", (c) => (c.runnerSha256["measure.mjs"] = "0".repeat(64))),
    identity("toolchain.nodeVersion", (c) => (c.toolchain.nodeVersion = "0.0.0-test")),
    {
      name: "a schemaVersion 2 contract",
      contract: (c) => {
        delete c.dependencyFingerprint;
        c.schemaVersion = 2;
      },
      message: /--reason cargo-removal needs a schemaVersion 3 contract, found schemaVersion 2/,
    },
    {
      name: "a contract pinning the new lock already",
      contract: (c) => (c.toolchain.cargoLockSha256 = NEW),
      message: /the contract pins Cargo\.lock [0-9a-f]{64}, not --expected-old-cargo-sha/,
    },
  ])("refuses $name end to end and writes nothing", async ({ contract: edit, message }) => {
    quiet();
    const contract = structuredClone(pinned);
    edit(contract);
    writeFileSync(at(CONTRACT), contractText(contract));
    const before = OUTPUTS.map(read);
    const stage = join(tempDir("zfb-removal-stage-"), "stage");
    await expect(main(argv({ stage }), repo.root)).rejects.toThrow(message);
    unchanged(before);
    expect(existsSync(stage)).toBe(false);
  });

  it("refuses an artifact measured at the old lock or on another platform", async () => {
    quiet();
    const before = OUTPUTS.map(read);
    await expect(
      main(argv({ artifact: linuxArtifact(undefined, OLD) }), repo.root),
    ).rejects.toThrow(/artifact Cargo\.lock hash differs from this checkout's Cargo\.lock/);
    await expect(
      main(
        argv({ artifact: linuxArtifact((p) => (p.platform = { os: "darwin", arch: "arm64" })) }),
        repo.root,
      ),
    ).rejects.toThrow(/not linux-x64; Darwin arm64 uses rebaseline-island-size-darwin\.mjs/);
    unchanged(before);
  });

  it.each([
    [
      "a missing --stage",
      { stage: undefined },
      "--reason cargo-removal re-pins both platforms together and only stages",
    ],
    ["a missing --source-sha", { "source-sha": undefined }, "required --source-sha"],
    [
      "a missing --removal-declaration",
      { "removal-declaration": undefined },
      "required --removal-declaration",
    ],
    ["a missing --old-closure", { "old-closure": undefined }, "required --old-closure"],
    ["a missing --old-cargo-ref", { "old-cargo-ref": undefined }, "required --old-cargo-ref"],
    [
      "a rewire declaration flag",
      { "rewire-declaration": "x.json" },
      "--rewire-declaration is only for --reason cargo-internal-rewire",
    ],
    [
      "a cargo-producer-lock-only flag",
      { "added-packages": "x@1.0.0" },
      "--added-packages is only for --reason cargo-producer-lock",
    ],
    [
      "a .. --new-cargo-ref path",
      { "new-cargo-ref": "HEAD:../Cargo.lock" },
      "--reason cargo-removal: --new-cargo-ref path must not contain a .. segment",
    ],
  ])("refuses %s in parseArgs, before reading anything", async (_name, extra, message) => {
    expect(() => parseArgs(argv(extra))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(argv(extra), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("keeps --removal-declaration to this reason and shares the closure arguments", () => {
    const base = ["--artifact", "dir", "--run-id", "1", "--job-id", "2", "--head-sha", HEAD_SHA];
    expect(() =>
      parseArgs([
        ...base,
        "--source-sha",
        SOURCE_SHA,
        "--stage",
        "s",
        "--reason",
        "cargo-internal-rewire",
        "--removal-declaration",
        "x",
      ]),
    ).toThrow("--removal-declaration is only for --reason cargo-removal");
    expect(() => parseArgs([...base, "--reason", "release-bump", "--old-closure", "x"])).toThrow(
      "--old-closure is only for --reason cargo-internal-rewire, cargo-removal",
    );
    expect(() => parseArgs([...base, "--reason", "release-bump", "--old-cargo-ref", "x"])).toThrow(
      "--old-cargo-ref is only for --reason cargo-producer-lock, cargo-internal-rewire, cargo-removal",
    );
  });

  it("refuses a stage whose staged file was tampered with", async () => {
    quiet();
    const stage = join(tempDir("zfb-removal-tamper-"), "stage");
    await main(argv({ stage }), repo.root);
    const staged = join(stage, CONTRACT);
    writeFileSync(staged, readFileSync(staged, "utf8").replace("cargo-removal-transition", "x"));
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      /the staged research\/v3-island-size\/decision-linux-x64\.json changed after staging/,
    );
    await expect(main(argv({ stage }), repo.root)).rejects.toThrow(/changed after staging/);
    expect(readdirSync(stage)).toContain("stage-manifest.json");
    unchanged(live());
  });
});

describe("cargo-removal prunedExternal (#4396)", () => {
  // serde_json keeps its version, source and checksum but loses the
  // `itoa 1.0.15` entry, the lock-level shape of an optional edge that only a
  // removed record's feature request enabled.
  const prunedLock = editRecord(newText, "serde_json@1.0.140", (body) =>
    body.replace('\ndependencies = [\n "itoa 1.0.15",\n]', ""),
  );
  const prune = (d, { oldText: old, newText: next, sha }) => {
    d.prunedExternal = [
      {
        id: "serde_json@1.0.140",
        oldSha256: sha(removalRecordOf(old, "serde_json@1.0.140").body),
        newSha256: sha(removalRecordOf(next, "serde_json@1.0.140").body),
        removedDependencies: ["itoa 1.0.15"],
      },
    ];
  };

  it("refuses the prune when it is not declared", () => {
    expect(() => prove({ newLock: prunedLock })).toThrow(
      /changed external records serde_json@1\.0\.140 \(an external record may not change/,
    );
  });

  it("admits a declared prune and records it", () => {
    const proof = prove({ newLock: prunedLock, declare: prune });
    expect(proof.prunedExternal).toEqual([
      {
        id: "serde_json@1.0.140",
        oldSha256: removalSha(removalRecordOf(oldText, "serde_json@1.0.140").body),
        newSha256: removalSha(removalRecordOf(prunedLock, "serde_json@1.0.140").body),
        removedDependencies: ["itoa 1.0.15"],
      },
    ]);
    expect(prove().prunedExternal).toBeUndefined();
  });

  it.each([
    [
      "a checksum change",
      editRecord(prunedLock, "serde_json@1.0.140", (body) =>
        body.replace(/checksum = "[0-9a-f]+"/, 'checksum = "' + "6".repeat(64) + '"'),
      ),
      prune,
      /declared pruned serde_json@1\.0\.140 moved a field other than dependencies/,
    ],
    [
      "a spelling rewrite",
      editRecord(newText, "serde_json@1.0.140", (body) => body.replace('"itoa 1.0.15"', '"itoa"')),
      (d, ctx) => {
        prune(d, ctx);
        d.prunedExternal[0].removedDependencies = ["itoa 1.0.15"];
      },
      /declared pruned serde_json@1\.0\.140 adds edges \["itoa"\]/,
    ],
    [
      "a wrong edge list",
      prunedLock,
      (d, ctx) => {
        prune(d, ctx);
        d.prunedExternal[0].removedDependencies = ["itoa 0.4.8"];
      },
      /declared pruned serde_json@1\.0\.140 removed edges \["itoa 1\.0\.15"\] differ/,
    ],
    [
      "an internal record",
      prunedLock,
      (d, ctx) => {
        prune(d, ctx);
        d.prunedExternal.push({ ...d.changed[0] });
        delete d.prunedExternal[1].manifest;
        d.changed.splice(0, 1);
      },
      /declared pruned zfb-js-host@0\.0\.0 is not an external/,
    ],
    [
      "a prune with no declared-removed dependent",
      prunedLock,
      (d, ctx) => {
        prune(d, ctx);
        d.removed = d.removed.filter((entry) => !entry.id.startsWith("serde_v8@"));
      },
      /declared pruned serde_json@1\.0\.140 has no declared-removed dependent/,
    ],
  ])("refuses %s", (_name, newLock, declare, message) => {
    expect(() => prove({ newLock, declare })).toThrow(message);
  });

  it("proves the committed Deno-exit declaration for both producer targets", () => {
    const dir = join(root, "research/v3-island-size/cargo-transitions");
    const liveOld = readFileSync(join(dir, "deno-exit-4303.old.lock"), "utf8");
    // The Deno-exit new lock is the #4429 transition's frozen old lock.
    const liveNew = readFileSync(join(dir, "rolldown-oxc-4429.old.lock"), "utf8");
    const declarationBytes = readFileSync(join(dir, "deno-exit-4303.json"));
    for (const target of ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"]) {
      const receipt = (side) => {
        const bytes = readFileSync(join(dir, `deno-exit-4303/closure-${side}-${target}.json`));
        return { receipt: JSON.parse(bytes), sha256: removalSha(bytes) };
      };
      const proof = proveCargoRemoval({
        contractSha: removalSha(liveOld),
        artifactSha: removalSha(liveNew),
        expectedOldSha: removalSha(liveOld),
        expectedNewSha: removalSha(liveNew),
        oldText: liveOld,
        newText: liveNew,
        oldLockSource: {
          commit: "2abb23825ff5a880d761e9c1cdce57b062a2b839",
          path: "Cargo.lock",
          sha256: removalSha(liveOld),
        },
        newLockSource: { sha256: removalSha(liveNew) },
        declaration: JSON.parse(declarationBytes),
        declarationSource: {
          path: "deno-exit-4303.json",
          sha256: removalSha(declarationBytes),
        },
        closures: { old: receipt("old"), new: receipt("new") },
        target,
        readManifest: (path) => readFileSync(join(root, path)),
      });
      expect(proof.prunedExternal.map((entry) => entry.id)).toEqual([
        "num-bigint@0.4.6",
        "tokio@1.52.3",
        "url@2.5.8",
      ]);
    }
  });

  it("refuses a malformed prunedExternal declaration", () => {
    for (const [edit, message] of [
      [(d) => (d.prunedExternal = []), /prunedExternal must be omitted rather than empty/],
      [(d) => (d.prunedExternal = {}), /prunedExternal must be an array/],
      [
        (d) =>
          (d.prunedExternal = [
            { id: "x@1.0.0", oldSha256: "1".repeat(64), newSha256: "2".repeat(64) },
          ]),
        /prunedExternal entries need exactly/,
      ],
      [
        (d) =>
          (d.prunedExternal = [
            {
              id: "x@1.0.0",
              oldSha256: "1".repeat(64),
              newSha256: "2".repeat(64),
              removedDependencies: [],
            },
          ]),
        /needs a nonempty removedDependencies array/,
      ],
      [
        (d) =>
          (d.prunedExternal = [
            {
              id: "deno_core@0.399.0",
              oldSha256: "1".repeat(64),
              newSha256: "2".repeat(64),
              removedDependencies: ["anyhow"],
            },
          ]),
        /repeats deno_core@0\.399\.0/,
      ],
    ]) {
      expect(() => checkRemovalDeclaration(removalDeclaration(edit))).toThrow(message);
    }
  });
});
