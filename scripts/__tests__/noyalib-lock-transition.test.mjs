import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import {
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";
import { NOYALIB_LOCK_TRANSITION } from "../security-lock-transitions.mjs";

import { noyalibLock } from "./fixtures/noyalib-lock-transition.mjs";
import { webpkiSecurityLock } from "./fixtures/webpki-security-transition.mjs";

const { oldText, newText, reviewedTransitionText } = noyalibLock();
const review = JSON.parse(reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
const added = ["noyalib-serde-yaml@0.0.58", "noyalib@0.0.58"];
const proof = (patch = {}) =>
  proveCargoProducerLock({
    oldText,
    newText,
    reviewedTransitionText,
    contractSha: review.oldSha,
    artifactSha: review.newSha,
    expectedOldSha: review.oldSha,
    expectedNewSha: review.newSha,
    oldLockSource: {
      commit: review.sourceSha,
      path: "Cargo.lock",
      sha256: review.oldSha,
    },
    added,
    ...patch,
  });

describe("exact noyalib 0.0.58 adoption", () => {
  it("pins the decision and both lock hashes in code", () => {
    expect(sha(reviewedTransitionText)).toBe(NOYALIB_LOCK_TRANSITION.decisionSha256);
    expect(sha(oldText)).toBe(NOYALIB_LOCK_TRANSITION.oldLockSha256);
    expect(sha(newText)).toBe(NOYALIB_LOCK_TRANSITION.newLockSha256);
    expect(review.oldSha).toBe(NOYALIB_LOCK_TRANSITION.oldLockSha256);
    expect(review.newSha).toBe(NOYALIB_LOCK_TRANSITION.newLockSha256);
  });
  it("starts from the lock the webpki transition produced", () => {
    expect(NOYALIB_LOCK_TRANSITION.oldLockSha256).toBe(sha(webpkiSecurityLock().newText));
  });
  it("leads to the committed Cargo.lock through the Deno-exit removal (#4396) and the #4429 oxc edge", () => {
    const declaration = (name) =>
      JSON.parse(
        readFileSync(
          new URL(`../../research/v3-island-size/cargo-transitions/${name}`, import.meta.url),
          "utf8",
        ),
      );
    const denoExit = declaration("deno-exit-4303.json");
    const oxcEdge = declaration("rolldown-oxc-4429.json");
    const live = readFileSync(new URL("../../Cargo.lock", import.meta.url), "utf8");
    expect(denoExit.oldCargoLockSha256).toBe(NOYALIB_LOCK_TRANSITION.newLockSha256);
    expect(oxcEdge.oldCargoLockSha256).toBe(denoExit.newCargoLockSha256);
    expect(sha(live)).toBe(oxcEdge.newCargoLockSha256);
  });
  it("admits only the approved pair and records the decision", () => {
    expect(proof()).toMatchObject({
      added,
      changed: [],
      removed: ["noyalib-serde-yaml@0.0.55", "noyalib@0.0.55"],
      review: {
        id: "noyalib-4268",
        sourceSha: review.sourceSha,
        manifestSha256: sha(reviewedTransitionText),
      },
    });
  });
  it("keeps the default proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(/removed records/);
  });
  it.each([
    ["version", (text) => text.replace('version = "0.0.58"', 'version = "0.0.57"')],
    ["checksum", (text) => text.replace("3fad8f1a", "00000000")],
    ["Windows edge", (text) => text.replace('"windows-sys 0.52.0"', '"windows-sys 0.59.0"')],
    ["header", (text) => `# changed\n${text}`],
    [
      "additional package",
      (text) => `${text}\n[[package]]\nname = "unapproved"\nversion = "1.0.0"\n`,
    ],
  ])("rejects a changed %s even when the artifact and operator agree", (_name, alter) => {
    const changed = alter(newText);
    expect(() =>
      proof({
        newText: changed,
        artifactSha: sha(changed),
        expectedNewSha: sha(changed),
      }),
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
  it("does not admit the webpki lock pair under the noyalib decision, or the reverse", () => {
    const webpki = webpkiSecurityLock();
    expect(() =>
      proveReviewedCargoTransition(reviewedTransitionText, webpki.oldText, webpki.newText),
    ).toThrow(/noyalib dependency transition requires the exact approved/);
    expect(() =>
      proveReviewedCargoTransition(webpki.reviewedTransitionText, oldText, newText),
    ).toThrow(/webpki security transition requires the exact approved/);
  });
  it.each(["newSha", "added", "decision"])("rejects caller-edited %s authorization", (key) => {
    const forged = { ...review, [key]: "unapproved" };
    expect(() => proof({ reviewedTransitionText: JSON.stringify(forged) })).toThrow(
      /approved bytes/,
    );
  });
  it("requires both replaced packages in --added-packages", () => {
    expect(() => proof({ added: ["noyalib@0.0.58"] })).toThrow(/added-packages differs/);
  });
  it("retains artifact, contract and source checks", () => {
    expect(() => proof({ artifactSha: "0".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "0".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ oldLockSource: undefined })).toThrow(/verified git blob/);
  });
});
