import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import {
  parseCargoLock,
  parseLockRecord,
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";
import {
  ESBUILD_REMOVAL_LOCK_TRANSITION,
  TOKIO_SECURITY_TRANSITION,
} from "../security-lock-transitions.mjs";

import { esbuildRemovalLock } from "./fixtures/esbuild-removal-transition.mjs";
import { tokioSecurityLock } from "./fixtures/tokio-security-transition.mjs";

const { oldText, newText, reviewedTransitionText } = esbuildRemovalLock();
const review = JSON.parse(reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
const added = ["zfb-bundler@0.0.0", "zfb-compiler-worker@0.0.0"];
const proof = (patch = {}) =>
  proveCargoProducerLock({
    oldText,
    newText,
    reviewedTransitionText,
    contractSha: review.oldSha,
    artifactSha: review.newSha,
    expectedOldSha: review.oldSha,
    expectedNewSha: review.newSha,
    oldLockSource: { commit: review.sourceSha, path: "Cargo.lock", sha256: review.oldSha },
    added,
    ...patch,
  });

// The record-level delta recomputed from the frozen bytes, in the shape the
// reviewed decision's `inventory` records it.
function recomputedInventory() {
  const before = parseCargoLock(oldText, "old Cargo.lock");
  const after = parseCargoLock(newText, "new Cargo.lock");
  const oldRecords = new Map(before.packages.map((record) => [record.id, record.body]));
  const newRecords = new Map(after.packages.map((record) => [record.id, record.body]));
  const deps = (body) => parseLockRecord(body, "record").dependencies ?? [];
  const inventory = { added: [], changed: [], removed: [] };
  for (const id of [...new Set([...oldRecords.keys(), ...newRecords.keys()])].sort()) {
    const old = oldRecords.get(id);
    const next = newRecords.get(id);
    if (old === next) continue;
    if (!old) inventory.added.push({ id, sha256: sha(next), dependencies: deps(next) });
    else if (!next) inventory.removed.push({ id, sha256: sha(old) });
    else {
      inventory.changed.push({
        id,
        beforeSha256: sha(old),
        afterSha256: sha(next),
        removedDependencies: deps(old).filter((entry) => !deps(next).includes(entry)),
        addedDependencies: deps(next).filter((entry) => !deps(old).includes(entry)),
      });
    }
  }
  return { before, after, oldRecords, newRecords, inventory };
}

describe("exact esbuild-removal lock move (#4635)", () => {
  it("pins the decision and both lock hashes in code", () => {
    expect(sha(reviewedTransitionText)).toBe(ESBUILD_REMOVAL_LOCK_TRANSITION.decisionSha256);
    expect(sha(oldText)).toBe(ESBUILD_REMOVAL_LOCK_TRANSITION.oldLockSha256);
    expect(sha(newText)).toBe(ESBUILD_REMOVAL_LOCK_TRANSITION.newLockSha256);
    expect(review.oldSha).toBe(ESBUILD_REMOVAL_LOCK_TRANSITION.oldLockSha256);
    expect(review.newSha).toBe(ESBUILD_REMOVAL_LOCK_TRANSITION.newLockSha256);
  });
  it("starts from the #4569 tokio replacement's new lock, byte for byte", () => {
    expect(ESBUILD_REMOVAL_LOCK_TRANSITION.oldLockSha256).toBe(
      TOKIO_SECURITY_TRANSITION.newLockSha256,
    );
    expect(oldText).toBe(tokioSecurityLock().newText);
  });
  it("matches the committed Cargo.lock byte for byte", () => {
    const live = readFileSync(new URL("../../Cargo.lock", import.meta.url), "utf8");
    expect(live).toBe(newText);
  });
  it("records the complete added/changed/removed inventory with record hashes", () => {
    const { before, after, inventory } = recomputedInventory();
    expect(after.header).toBe(before.header);
    expect(after.other).toEqual(before.other);
    expect(review.inventory).toEqual(inventory);
    expect(review.added).toEqual(inventory.added.map((row) => row.id).sort());
    expect(review.changed).toEqual(inventory.changed.map((row) => row.id).sort());
    expect(review.removed).toEqual(inventory.removed.map((row) => row.id).sort());
    expect(before.packages).toHaveLength(676);
    expect(after.packages).toHaveLength(674);
  });
  it("adds and changes only unpublished internal records, and removes only tar and xattr externally", () => {
    const { oldRecords, newRecords } = recomputedInventory();
    const internal = (body) => {
      const record = parseLockRecord(body, "record");
      return record.source === undefined && record.checksum === undefined;
    };
    for (const id of [...review.added, ...review.changed]) {
      expect(internal(newRecords.get(id)), id).toBe(true);
    }
    const externalRemovals = review.removed.filter((id) => !internal(oldRecords.get(id)));
    expect(externalRemovals).toEqual(["tar@0.4.45", "xattr@1.6.1"]);
    for (const [id, body] of newRecords) {
      if (!internal(body)) expect(oldRecords.get(id), id).toBe(body);
    }
  });
  it("admits only the approved pair and records the decision", () => {
    expect(proof()).toMatchObject({
      added,
      changed: review.changed,
      removed: ["tar@0.4.45", "xattr@1.6.1", "zfb-binfetch@0.0.0", "zfb-rolldown-prototype@0.0.0"],
      review: {
        id: "esbuild-removal-4635",
        sourceSha: review.sourceSha,
        manifestSha256: sha(reviewedTransitionText),
      },
    });
  });
  it("keeps the default proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(/changed records/);
  });
  it.each([
    ["internal edge", (text) => text.replace(' "zfb-compiler-worker",\n', "")],
    ["checksum", (text) => text.replace("b52fd325", "00000000")],
    ["header", (text) => `# changed\n${text}`],
    [
      "additional package",
      (text) => `${text}\n[[package]]\nname = "unapproved"\nversion = "1.0.0"\n`,
    ],
  ])("rejects a changed %s even when the artifact and operator agree", (_name, alter) => {
    const changed = alter(newText);
    expect(changed).not.toBe(newText);
    expect(() =>
      proof({ newText: changed, artifactSha: sha(changed), expectedNewSha: sha(changed) }),
    ).toThrow(/exact approved old\/new/);
  });
  it("rejects other predecessors and reversal", () => {
    expect(() =>
      proveReviewedCargoTransition(reviewedTransitionText, `${oldText}\n`, newText),
    ).toThrow(/exact approved/);
    expect(() => proveReviewedCargoTransition(reviewedTransitionText, newText, oldText)).toThrow(
      /exact approved/,
    );
  });
  it("does not admit the tokio lock pair under this decision, or the reverse", () => {
    const tokio = tokioSecurityLock();
    expect(() =>
      proveReviewedCargoTransition(reviewedTransitionText, tokio.oldText, tokio.newText),
    ).toThrow(/esbuild-removal milestone transition requires the exact approved/);
    expect(() =>
      proveReviewedCargoTransition(tokio.reviewedTransitionText, oldText, newText),
    ).toThrow(/tokio security transition requires the exact approved/);
  });
  it.each(["newSha", "added", "removed", "inventory", "decision"])(
    "rejects caller-edited %s authorization",
    (key) => {
      const forged = { ...review, [key]: "unapproved" };
      expect(() => proof({ reviewedTransitionText: JSON.stringify(forged) })).toThrow(
        /approved bytes/,
      );
    },
  );
  it("requires both new internal records in --added-packages", () => {
    expect(() => proof({ added: ["zfb-bundler@0.0.0"] })).toThrow(/added-packages differs/);
  });
  it("retains artifact, contract and source checks", () => {
    expect(() => proof({ artifactSha: "0".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "0".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ oldLockSource: undefined })).toThrow(/verified git blob/);
  });
});
