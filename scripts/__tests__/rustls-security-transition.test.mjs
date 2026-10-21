import { createHash } from "node:crypto";
import { describe, expect, it } from "vite-plus/test";
import {
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";

import { rustlsSecurityLock } from "./fixtures/rustls-security-transition.mjs";

const { oldText, newText, reviewedTransitionText } = rustlsSecurityLock();
const review = JSON.parse(reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
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
    added: ["rustls@0.23.46"],
    ...patch,
  });

describe("exact rustls security replacement", () => {
  it("admits only the approved pair and records the decision", () => {
    expect(sha(oldText)).toBe(review.oldSha);
    expect(sha(newText)).toBe(review.newSha);
    expect(proof()).toMatchObject({
      added: ["rustls@0.23.46"],
      changed: [],
      removed: ["rustls@0.23.45"],
      review: {
        id: "rustls-security-4314",
        sourceSha: review.sourceSha,
        manifestSha256: sha(reviewedTransitionText),
      },
    });
  });
  it("keeps the default proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(
      /removed records rustls@0.23.45/,
    );
  });
  it.each([
    ["version", (text) => text.replace('version = "0.23.46"', 'version = "0.23.47"')],
    ["checksum", (text) => text.replace("48e13bd8", "00000000")],
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
  it.each(["newSha", "added", "decision", "nativeMeasurement"])(
    "rejects caller-edited %s authorization",
    (key) => {
      const forged = { ...review, [key]: "unapproved" };
      expect(() => proof({ reviewedTransitionText: JSON.stringify(forged) })).toThrow(
        /approved bytes/,
      );
    },
  );
  it("retains artifact, contract, source and allowlist checks", () => {
    expect(() => proof({ artifactSha: "0".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "0".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ oldLockSource: undefined })).toThrow(/verified git blob/);
    expect(() => proof({ added: [] })).toThrow(/added-packages differs/);
  });
});
