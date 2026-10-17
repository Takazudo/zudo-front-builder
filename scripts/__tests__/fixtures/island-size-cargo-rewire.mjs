// Test fixture — synthetic internal Cargo.lock rewire, not native evidence
// (#4258): the five-record locks in fixtures/cargo-internal-rewire/, the
// workspace manifests that prove `zfb-js-host` and `zfb-render` internal, the
// reviewed declaration derived from the exact record bytes, and producer
// closure receipts for either platform target. Nothing here is a live contract,
// a real transition or a measured artifact.
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { parseCargoLock, parseLockRecord } from "../../rebaseline-island-size-linux.mjs";

const lockDir = join(dirname(fileURLToPath(import.meta.url)), "cargo-internal-rewire");

export const REWIRE_PRODUCER = "zfb";
export const LINUX_TARGET = "x86_64-unknown-linux-gnu";
export const DARWIN_TARGET = "aarch64-apple-darwin";

function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function rewireLocks() {
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  return { oldText, newText, OLD: sha(oldText), NEW: sha(newText) };
}

// The exact bytes of one `[[package]]` record in `lockText`.
export function recordOf(lockText, id) {
  const record = parseCargoLock(lockText, "fixture").packages.find((r) => r.id === id);
  if (!record) throw new Error("test fixture: no record " + id);
  return record;
}

// The workspace manifests the declaration references, written under `root`.
// `edit` may rewrite a manifest's text by path before it is written.
export const REWIRE_MANIFESTS = Object.freeze({
  "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n',
  "crates/zfb-js-host/Cargo.toml":
    '[package]\nname = "zfb-js-host"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n',
  "crates/zfb-render/Cargo.toml":
    '[package]\nname = "zfb-render"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n',
});

export function writeRewireManifests(root, edit = (_path, text) => text) {
  for (const [path, text] of Object.entries(REWIRE_MANIFESTS)) {
    const file = join(root, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, edit(path, text));
  }
}

// The reviewed declaration for the fixture transition, derived from the exact
// record bytes so the hashes always describe old.lock/new.lock; `edit` mutates
// it (a wrong hash, an undeclared edge) before it is returned.
export function rewireDeclaration(edit = () => {}) {
  const { oldText, newText, OLD, NEW } = rewireLocks();
  const added = recordOf(newText, "zfb-js-host@0.0.0");
  const changedOld = recordOf(oldText, "zfb-render@0.0.0");
  const changedNew = recordOf(newText, "zfb-render@0.0.0");
  const declaration = {
    schemaVersion: 1,
    producer: REWIRE_PRODUCER,
    oldCargoLockSha256: OLD,
    newCargoLockSha256: NEW,
    added: [
      {
        id: "zfb-js-host@0.0.0",
        manifest: "crates/zfb-js-host/Cargo.toml",
        sha256: sha(added.body),
        dependencies: parseLockRecord(added.body, "fixture").dependencies,
      },
    ],
    changed: [
      {
        id: "zfb-render@0.0.0",
        manifest: "crates/zfb-render/Cargo.toml",
        oldSha256: sha(changedOld.body),
        newSha256: sha(changedNew.body),
        removedDependencies: ["sha2"],
        addedDependencies: ["zfb-js-host"],
      },
    ],
    closure: { added: ["zfb-js-host@0.0.0"], changed: [] },
  };
  edit(declaration);
  return declaration;
}

const REGISTRY = "registry+https://github.com/rust-lang/crates.io-index";

// The producer closure rows of the fixture locks: the producer `zfb`, its two
// path crates, the external `anyhow`/`sha2` rows and, at the new lock, the
// internal `zfb-js-host` row. Features are fixed literals.
export function closureRows(side) {
  const rows = [
    { name: "anyhow", version: "1.0.100", source: REGISTRY, features: ["default", "std"] },
    { name: "sha2", version: "0.10.9", source: REGISTRY, features: ["default", "std"] },
    { name: "zfb", version: "0.0.0", source: "path+crates/zfb", features: ["default"] },
    {
      name: "zfb-render",
      version: "0.0.0",
      source: "path+crates/zfb-render",
      features: ["default", "embed_v8"],
    },
    { name: "zudo-wind", version: "0.0.0", source: "path+crates/zudo-wind", features: [] },
  ];
  if (side === "new") {
    rows.splice(3, 0, {
      name: "zfb-js-host",
      version: "0.0.0",
      source: "path+crates/zfb-js-host",
      features: ["default", "services"],
    });
  }
  return rows;
}

// A closure receipt in the shape scripts/record-cargo-producer-closure.mjs
// writes, bound to the fixture lock of `side`; `edit` mutates it first.
export function closureReceipt(side, target, edit = () => {}) {
  const { OLD, NEW } = rewireLocks();
  const receipt = {
    schemaVersion: 1,
    producer: REWIRE_PRODUCER,
    target,
    edges: "normal,build",
    cargoLockSha256: side === "old" ? OLD : NEW,
    command: [
      "cargo",
      "metadata",
      "--locked",
      "--format-version",
      "1",
      "--filter-platform",
      target,
    ],
    cargo: "test fixture: no cargo ran",
    rows: closureRows(side),
  };
  edit(receipt);
  return receipt;
}

// Writes the declaration and both closure receipts under `dir` and returns the
// CLI argument values naming them. `edits` may rewrite each document.
export function writeRewireInputs(dir, target, edits = {}) {
  mkdirSync(dir, { recursive: true });
  const files = {
    "rewire-declaration": ["declaration.json", rewireDeclaration(edits.declaration)],
    "old-closure": ["closure-old.json", closureReceipt("old", target, edits.oldClosure)],
    "new-closure": ["closure-new.json", closureReceipt("new", target, edits.newClosure)],
  };
  const values = {};
  for (const [key, [name, document]] of Object.entries(files)) {
    const path = join(dir, name);
    writeFileSync(path, JSON.stringify(document, null, 2) + "\n");
    values[key] = path;
  }
  return values;
}

export { sha as rewireSha };
