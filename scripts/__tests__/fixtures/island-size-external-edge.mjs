// Synthetic schema-2 proof inputs only. No native measurements or live pins.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  closureReceipt as internalReceipt,
  recordOf,
  rewireLocks,
  rewireSha as sha,
} from "./island-size-cargo-rewire.mjs";

export function externalLocks() {
  const oldText = rewireLocks().newText;
  const newText = oldText.replace(
    'name = "zfb-render"\nversion = "0.0.0"\ndependencies = [\n "anyhow",',
    'name = "zfb-render"\nversion = "0.0.0"\ndependencies = [\n "anyhow",\n "sha2",',
  );
  return { oldText, newText, OLD: sha(oldText), NEW: sha(newText) };
}

export function externalDeclaration(edit = () => {}) {
  const { oldText, newText, OLD, NEW } = externalLocks();
  const declaration = {
    schemaVersion: 2,
    producer: "zfb",
    oldCargoLockSha256: OLD,
    newCargoLockSha256: NEW,
    added: [],
    changed: [
      {
        id: "zfb-render@0.0.0",
        manifest: "crates/zfb-render/Cargo.toml",
        oldSha256: sha(recordOf(oldText, "zfb-render@0.0.0").body),
        newSha256: sha(recordOf(newText, "zfb-render@0.0.0").body),
        removedDependencies: [],
        addedDependencies: ["sha2"],
      },
    ],
    externalEdges: [
      {
        from: "zfb-render@0.0.0",
        dependency: "sha2",
        to: "sha2@0.10.9",
        sha256: sha(recordOf(oldText, "sha2@0.10.9").body),
      },
    ],
    closure: { added: [], changed: [] },
  };
  edit(declaration);
  return declaration;
}

export function externalReceipt(side, target, edit = () => {}) {
  const { OLD, NEW } = externalLocks();
  const receipt = internalReceipt("new", target);
  receipt.cargoLockSha256 = side === "old" ? OLD : NEW;
  edit(receipt);
  return receipt;
}

export function writeExternalInputs(dir, target, edits = {}) {
  mkdirSync(dir, { recursive: true });
  const files = {
    "rewire-declaration": ["declaration.json", externalDeclaration(edits.declaration)],
    "old-closure": ["closure-old.json", externalReceipt("old", target, edits.oldClosure)],
    "new-closure": ["closure-new.json", externalReceipt("new", target, edits.newClosure)],
  };
  return Object.fromEntries(
    Object.entries(files).map(([key, [name, document]]) => {
      const path = join(dir, name);
      writeFileSync(path, JSON.stringify(document, null, 2) + "\n");
      return [key, path];
    }),
  );
}
