// Test fixture — synthetic removal-only Cargo.lock transition, not native
// evidence (#4388): the eleven-record locks in fixtures/cargo-removal/, the
// workspace manifests that prove the changed records internal, the reviewed
// declaration derived from the exact record bytes, and default-feature
// producer closure receipts for either platform target. Nothing here is a live
// contract, a real transition or a measured artifact.
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { parseCargoLock } from "../../rebaseline-island-size-linux.mjs";

const lockDir = join(dirname(fileURLToPath(import.meta.url)), "cargo-removal");

export const REMOVAL_PRODUCER = "zfb";

function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function removalLocks() {
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  return { oldText, newText, OLD: sha(oldText), NEW: sha(newText) };
}

// The exact bytes of one `[[package]]` record in `lockText`.
export function removalRecordOf(lockText, id) {
  const record = parseCargoLock(lockText, "fixture").packages.find((r) => r.id === id);
  if (!record) throw new Error("test fixture: no record " + id);
  return record;
}

// `lockText` without the `[[package]]` record `id` (and its separating blank line).
export function withoutRecord(lockText, id) {
  const { body } = removalRecordOf(lockText, id);
  const at = lockText.indexOf("\n" + body);
  if (at < 0) throw new Error("test fixture: cannot cut " + id);
  return lockText.slice(0, at) + lockText.slice(at + body.length + 1);
}

// `lockText` with record `id`'s exact bytes passed through `edit`.
export function editRecord(lockText, id, edit) {
  const { body } = removalRecordOf(lockText, id);
  return lockText.replace(body, edit(body));
}

// The workspace manifests the declaration references, written under `root`.
export const REMOVAL_MANIFESTS = Object.freeze({
  "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n',
  "crates/zfb-js-host/Cargo.toml":
    '[package]\nname = "zfb-js-host"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n',
  "crates/zfb-render/Cargo.toml":
    '[package]\nname = "zfb-render"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n',
  "crates/zudo-wind/Cargo.toml":
    '[package]\nname = "zudo-wind"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n',
});

export function writeRemovalManifests(root, edit = (_path, text) => text) {
  for (const [path, text] of Object.entries(REMOVAL_MANIFESTS)) {
    const file = join(root, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, edit(path, text));
  }
}

// The reviewed declaration for a removal from `oldText` to `newText` (the
// fixture locks by default), derived from the exact record bytes; `edit`
// mutates it (a wrong hash, an undeclared removal) before it is returned.
export function removalDeclaration(edit = () => {}, locks = removalLocks()) {
  const { oldText, newText } = locks;
  const changed = (id, manifest, removedDependencies) => ({
    id,
    manifest,
    oldSha256: sha(removalRecordOf(oldText, id).body),
    newSha256: sha(removalRecordOf(newText, id).body),
    removedDependencies,
  });
  const declaration = {
    schemaVersion: 1,
    producer: REMOVAL_PRODUCER,
    oldCargoLockSha256: sha(oldText),
    newCargoLockSha256: sha(newText),
    removed: ["deno_core@0.399.0", "serde_v8@0.250.0"].map((id) => ({
      id,
      sha256: sha(removalRecordOf(oldText, id).body),
    })),
    changed: [
      changed("zfb-js-host@0.0.0", "crates/zfb-js-host/Cargo.toml", ["deno_core"]),
      changed("zfb-render@0.0.0", "crates/zfb-render/Cargo.toml", ["deno_core"]),
    ],
    closure: { removed: [] },
  };
  edit(declaration, { oldText, newText, sha, changed });
  return declaration;
}

const REGISTRY = "registry+https://github.com/rust-lang/crates.io-index";

// The default-feature producer closure rows, identical at both fixture locks:
// `deno_core`/`serde_v8` were never in it and `serde_json`/`itoa 1.0.15` are
// reached only over a dev edge. Features are fixed literals.
export function removalClosureRows() {
  return [
    { name: "anyhow", version: "1.0.100", source: REGISTRY, features: ["default", "std"] },
    { name: "itoa", version: "0.4.8", source: REGISTRY, features: [] },
    { name: "sha2", version: "0.10.9", source: REGISTRY, features: ["default", "std"] },
    { name: "zfb", version: "0.0.0", source: "path+crates/zfb", features: ["default"] },
    {
      name: "zfb-js-host",
      version: "0.0.0",
      source: "path+crates/zfb-js-host",
      features: ["default"],
    },
    {
      name: "zfb-render",
      version: "0.0.0",
      source: "path+crates/zfb-render",
      features: ["default"],
    },
    { name: "zudo-wind", version: "0.0.0", source: "path+crates/zudo-wind", features: [] },
  ];
}

// A closure receipt in the shape scripts/record-cargo-producer-closure.mjs
// writes, bound to `lockText` (the fixture lock of `side` by default); `edit`
// mutates it first.
export function removalReceipt(side, target, edit = () => {}, lockText = undefined) {
  const { oldText, newText } = removalLocks();
  const receipt = {
    schemaVersion: 1,
    producer: REMOVAL_PRODUCER,
    target,
    edges: "normal,build",
    cargoLockSha256: sha(lockText ?? (side === "old" ? oldText : newText)),
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
    rows: removalClosureRows(),
  };
  edit(receipt);
  return receipt;
}

// Writes the declaration and both closure receipts under `dir` and returns the
// CLI argument values naming them. `edits` may rewrite each document.
export function writeRemovalInputs(dir, target, edits = {}) {
  mkdirSync(dir, { recursive: true });
  const files = {
    "removal-declaration": ["declaration.json", removalDeclaration(edits.declaration)],
    "old-closure": ["closure-old.json", removalReceipt("old", target, edits.oldClosure)],
    "new-closure": ["closure-new.json", removalReceipt("new", target, edits.newClosure)],
  };
  const values = {};
  for (const [key, [name, document]] of Object.entries(files)) {
    const path = join(dir, name);
    writeFileSync(path, JSON.stringify(document, null, 2) + "\n");
    values[key] = path;
  }
  return values;
}

export { sha as removalSha };
