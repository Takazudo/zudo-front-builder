#!/usr/bin/env node
// Structural dry proof of the proposed #4307 Cargo.lock rewire. It runs the
// existing proveCargoInternalRewire over the real old/new locks, the proposed
// declaration and the old/new closure receipts, for each producer target.
// It never reads or writes contracts beyond their pinned lock SHA, stages nothing
// and measures no artifact: artifactSha is the expected new lock, labeled as
// substituted. Fresh native artifacts and the paired stage/apply stay separate.
//
// Usage (repo root, clean checkout of the candidate):
//   node prototypes/direct-v8/admission/prove-rewire.mjs --old-ref <commit> \
//     --receipts <dir with {old,new}-<target>.receipt>
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import {
  cargoRewireText,
  proveCargoInternalRewire,
} from "../../../scripts/rebaseline-island-size-linux.mjs";

const TARGETS = {
  "x86_64-unknown-linux-gnu": "research/v3-island-size/decision-linux-x64.json",
  "aarch64-apple-darwin": "research/v3-island-size/decision.json",
};
const DECLARATION = "prototypes/direct-v8/admission/rewire-declaration.json";
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const git = (...args) => execFileSync("git", args, { maxBuffer: 1 << 28 });

const { values } = parseArgs({
  options: { "old-ref": { type: "string" }, receipts: { type: "string" } },
});
if (!values["old-ref"] || !values.receipts) {
  console.error("usage: prove-rewire.mjs --old-ref <commit> --receipts <dir>");
  process.exit(2);
}
const oldCommit = git("rev-parse", values["old-ref"] + "^{commit}")
  .toString()
  .trim();
const head = git("rev-parse", "HEAD").toString().trim();
if (git("status", "--porcelain", "--untracked-files=all").toString().trim()) {
  console.error("refusing: the candidate checkout is not clean");
  process.exit(2);
}
const oldText = git("show", `${oldCommit}:Cargo.lock`);
const newText = readFileSync("Cargo.lock");
const declarationBytes = readFileSync(DECLARATION);
let failed = false;
for (const [target, contractPath] of Object.entries(TARGETS)) {
  const contract = JSON.parse(readFileSync(contractPath, "utf8"));
  const closures = {};
  for (const side of ["old", "new"]) {
    const bytes = readFileSync(join(values.receipts, `${side}-${target}.receipt`));
    closures[side] = { receipt: JSON.parse(bytes), sha256: sha(bytes) };
  }
  try {
    const transition = proveCargoInternalRewire({
      contractSha: contract.toolchain.cargoLockSha256,
      artifactSha: sha(newText),
      expectedOldSha: contract.toolchain.cargoLockSha256,
      expectedNewSha: sha(newText),
      oldText,
      newText,
      oldLockSource: { commit: oldCommit, path: "Cargo.lock", sha256: sha(oldText) },
      newLockSource: { commit: head, path: "Cargo.lock", sha256: sha(newText) },
      declaration: JSON.parse(declarationBytes),
      declarationSource: { path: DECLARATION, sha256: sha(declarationBytes) },
      closures,
      target,
      readManifest: (manifest) => git("show", `${head}:${manifest}`),
      manifestCommit: head,
    });
    console.log(`PASS ${target} (structural; artifact identity substituted, not measured)`);
    console.log(cargoRewireText(transition));
  } catch (error) {
    failed = true;
    console.log(`FAIL ${target}: ${error.message}`);
  }
}
process.exit(failed ? 1 : 0);
