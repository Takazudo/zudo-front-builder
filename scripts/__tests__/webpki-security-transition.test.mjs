import { createHash } from "node:crypto";
import { describe, expect, it } from "vite-plus/test";
import {
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";
import { WEBPKI_SECURITY_TRANSITION } from "../security-lock-transitions.mjs";

import { rustlsSecurityLock } from "./fixtures/rustls-security-transition.mjs";
import { webpkiSecurityLock } from "./fixtures/webpki-security-transition.mjs";

const { oldText, newText, reviewedTransitionText } = webpkiSecurityLock();
const review = JSON.parse(reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
const added = ["rustls-webpki@0.103.16", "webpki-roots@1.0.10"];
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

describe("exact webpki security replacement", () => {
  it("pins the decision and both lock hashes in code", () => {
    expect(sha(reviewedTransitionText)).toBe(WEBPKI_SECURITY_TRANSITION.decisionSha256);
    expect(sha(oldText)).toBe(WEBPKI_SECURITY_TRANSITION.oldLockSha256);
    expect(sha(newText)).toBe(WEBPKI_SECURITY_TRANSITION.newLockSha256);
    expect(review.oldSha).toBe(WEBPKI_SECURITY_TRANSITION.oldLockSha256);
    expect(review.newSha).toBe(WEBPKI_SECURITY_TRANSITION.newLockSha256);
  });
  it("admits only the approved pair and records the decision", () => {
    expect(proof()).toMatchObject({
      added,
      changed: [],
      removed: ["rustls-webpki@0.103.15", "webpki-roots@1.0.7"],
      review: {
        id: "webpki-security-4314",
        sourceSha: review.sourceSha,
        manifestSha256: sha(reviewedTransitionText),
      },
    });
  });
  it("keeps the default proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(/removed records/);
  });
  it.each([
    ["version", (text) => text.replace('version = "1.0.10"', 'version = "1.0.11"')],
    ["checksum", (text) => text.replace("4443b1b6", "00000000")],
    ["Windows edge", (text) => text.replace('"windows-sys 0.52.0"', '"windows-sys 0.59.0"')],
    ["header", (text) => `# changed\n${text}`],
    [
      "additional package",
      (text) => `${text}\n[[package]]\nname = "unapproved"\nversion = "1.0.0"\n`,
    ],
  ])("rejects a changed %s even when the artifact and operator agree", (_name, alter) => {
    const changed = alter(newText);
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
  it("does not admit the rustls lock pair under the webpki decision, or the reverse", () => {
    const rustls = rustlsSecurityLock();
    expect(() =>
      proveReviewedCargoTransition(reviewedTransitionText, rustls.oldText, rustls.newText),
    ).toThrow(/webpki security transition requires the exact approved/);
    expect(() =>
      proveReviewedCargoTransition(rustls.reviewedTransitionText, oldText, newText),
    ).toThrow(/rustls security transition requires the exact approved/);
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
  it("requires both replaced packages in --added-packages", () => {
    expect(() => proof({ added: ["rustls-webpki@0.103.16"] })).toThrow(/added-packages differs/);
  });
  it("retains artifact, contract and source checks", () => {
    expect(() => proof({ artifactSha: "0".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "0".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ oldLockSource: undefined })).toThrow(/verified git blob/);
  });
});
