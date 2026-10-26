import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import {
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";
import { TOKIO_SECURITY_TRANSITION } from "../security-lock-transitions.mjs";

import { noyalibLock } from "./fixtures/noyalib-lock-transition.mjs";
import { tokioSecurityLock } from "./fixtures/tokio-security-transition.mjs";

const { oldText, newText, reviewedTransitionText } = tokioSecurityLock();
const review = JSON.parse(reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
const added = ["tokio@1.54.1"];
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

describe("exact tokio security replacement (#4569)", () => {
  it("pins the decision and both lock hashes in code", () => {
    expect(sha(reviewedTransitionText)).toBe(TOKIO_SECURITY_TRANSITION.decisionSha256);
    expect(sha(oldText)).toBe(TOKIO_SECURITY_TRANSITION.oldLockSha256);
    expect(sha(newText)).toBe(TOKIO_SECURITY_TRANSITION.newLockSha256);
    expect(review.oldSha).toBe(TOKIO_SECURITY_TRANSITION.oldLockSha256);
    expect(review.newSha).toBe(TOKIO_SECURITY_TRANSITION.newLockSha256);
  });
  it("starts from the #4429 oxc edge's new lock", () => {
    const oxcEdge = JSON.parse(
      readFileSync(
        new URL(
          "../../research/v3-island-size/cargo-transitions/rolldown-oxc-4429.json",
          import.meta.url,
        ),
        "utf8",
      ),
    );
    expect(TOKIO_SECURITY_TRANSITION.oldLockSha256).toBe(oxcEdge.newCargoLockSha256);
  });
  it("matches the committed Cargo.lock byte for byte", () => {
    const live = readFileSync(new URL("../../Cargo.lock", import.meta.url), "utf8");
    expect(sha(live)).toBe(TOKIO_SECURITY_TRANSITION.newLockSha256);
  });
  it("admits only the approved pair and records the decision", () => {
    expect(proof()).toMatchObject({
      added,
      changed: [],
      removed: ["tokio@1.52.3"],
      review: {
        id: "tokio-security-4569",
        sourceSha: review.sourceSha,
        manifestSha256: sha(reviewedTransitionText),
      },
    });
  });
  it("keeps the default proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(/removed records/);
  });
  it.each([
    ["version", (text) => text.replace('version = "1.54.1"', 'version = "1.54.0"')],
    ["checksum", (text) => text.replace("b52fd325", "00000000")],
    ["Windows edge", (text) => text.replace('"windows-sys 0.61.2"', '"windows-sys 0.59.0"')],
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
  it("does not admit the noyalib lock pair under the tokio decision, or the reverse", () => {
    const noyalib = noyalibLock();
    expect(() =>
      proveReviewedCargoTransition(reviewedTransitionText, noyalib.oldText, noyalib.newText),
    ).toThrow(/tokio security transition requires the exact approved/);
    expect(() =>
      proveReviewedCargoTransition(noyalib.reviewedTransitionText, oldText, newText),
    ).toThrow(/noyalib dependency transition requires the exact approved/);
  });
  it.each(["newSha", "added", "decision", "advisories"])(
    "rejects caller-edited %s authorization",
    (key) => {
      const forged = { ...review, [key]: "unapproved" };
      expect(() => proof({ reviewedTransitionText: JSON.stringify(forged) })).toThrow(
        /approved bytes/,
      );
    },
  );
  it("requires the replaced package in --added-packages", () => {
    expect(() => proof({ added: [] })).toThrow(/added-packages differs/);
  });
  it("retains artifact, contract and source checks", () => {
    expect(() => proof({ artifactSha: "0".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "0".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ oldLockSource: undefined })).toThrow(/verified git blob/);
  });
});
