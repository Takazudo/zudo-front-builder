import { createHash } from "node:crypto";
import { describe, expect, it } from "vite-plus/test";
import {
  cargoTransitionMarkdown,
  cargoTransitionText,
  checkCargoProducerLockArgs,
  proveCargoProducerLock,
  proveReviewedCargoTransition,
} from "../rebaseline-island-size-linux.mjs";
import { reviewedCargoLock } from "./fixtures/reviewed-cargo-transition.mjs";

const lock = reviewedCargoLock();
const review = JSON.parse(lock.reviewedTransitionText);
const sha = (text) => createHash("sha256").update(text).digest("hex");
const proof = (patch = {}) =>
  proveCargoProducerLock({
    ...lock,
    contractSha: lock.expectedOldSha,
    artifactSha: lock.expectedNewSha,
    ...patch,
  });

describe("reviewed non-additive producer lock transition", () => {
  it("binds the exact reviewed records and records the manifest identity", () => {
    const result = proof();
    expect(result.added).toHaveLength(111);
    expect(result.changed).toHaveLength(46);
    expect(result.removed).toHaveLength(16);
    expect(result.review).toEqual({
      id: review.id,
      manifestSha256: sha(lock.reviewedTransitionText),
      sourceSha: review.review.sourceSha,
    });
    for (const text of [cargoTransitionText(result), cargoTransitionMarkdown(result)]) {
      expect(text).toContain("non-additive");
      expect(text).toContain("46 changed records");
      expect(text).not.toContain("every previous");
    }
  });

  it("keeps the ordinary proof additive-only", () => {
    expect(() => proof({ reviewedTransitionText: undefined })).toThrow(
      /not the reviewed additive transition/,
    );
  });

  it("rejects unreviewed lock bytes even when operator and artifact agree on their hash", () => {
    const newText = lock.newText.replace('name = "anyhow"', 'name = "unreviewed-anyhow"');
    expect(() =>
      proof({ newText, expectedNewSha: sha(newText), artifactSha: sha(newText) }),
    ).toThrow(/reviewed Cargo transition old\/new hashes/);
    const changedReview = { ...review, newSha: sha(newText) };
    expect(() =>
      proof({
        newText,
        expectedNewSha: sha(newText),
        artifactSha: sha(newText),
        reviewedTransitionText: JSON.stringify(changedReview),
      }),
    ).toThrow(/unsupported reviewed Cargo transition/);
  });

  it.each(["added", "changed", "removed"])("requires the complete exact %s inventory", (kind) => {
    const edited = structuredClone(review);
    edited.delta[kind].pop();
    expect(() => proof({ reviewedTransitionText: JSON.stringify(edited) })).toThrow(
      /inventory differs/,
    );
    const forged = structuredClone(review);
    forged.delta[kind][0].beforeSha256 = "f".repeat(64);
    expect(() => proof({ reviewedTransitionText: JSON.stringify(forged) })).toThrow(
      /inventory differs/,
    );
  });

  it.each(["sharedReplacements", "windowsEdges", "featureGraphs"])(
    "requires the full %s review",
    (key) => {
      const edited = structuredClone(review);
      edited.review[key].pop();
      expect(() => proof({ reviewedTransitionText: JSON.stringify(edited) })).toThrow(
        /reviewed Cargo transition needs/,
      );
    },
  );

  it("refuses missing native gate, wrong hashes, unexpected schema and incomplete addition allowlist", () => {
    const edited = structuredClone(review);
    edited.review.nativeMeasurement = "waived";
    expect(() => proof({ reviewedTransitionText: JSON.stringify(edited) })).toThrow(
      /native measurement gate/,
    );
    expect(() => proof({ artifactSha: "f".repeat(64) })).toThrow(/artifact measured/);
    expect(() => proof({ contractSha: "f".repeat(64) })).toThrow(/contract pins/);
    expect(() => proof({ added: lock.added.slice(1) })).toThrow(/added-packages differs/);
    expect(() =>
      proof({ reviewedTransitionText: JSON.stringify({ ...review, allowAny: true }) }),
    ).toThrow(/unsupported reviewed Cargo transition/);
    expect(() => proveReviewedCargoTransition("{", lock.oldText, lock.newText)).toThrow(
      /invalid reviewed/,
    );
  });

  it("permits the extra CLI evidence only for cargo-producer-lock", () => {
    const args = {
      reason: "cargo-producer-lock",
      "expected-old-cargo-sha": lock.expectedOldSha,
      "expected-new-cargo-sha": lock.expectedNewSha,
      "old-cargo-ref": "HEAD",
      "old-cargo-lock": "old.lock",
      "added-packages": lock.added.join(","),
      "reviewed-cargo-transition": "review.json",
    };
    expect(() => checkCargoProducerLockArgs(args)).not.toThrow();
    expect(() =>
      checkCargoProducerLockArgs({
        reason: "runtime-identity",
        "reviewed-cargo-transition": "review.json",
      }),
    ).toThrow(/only for --reason cargo-producer-lock/);
  });

  it.each(["graph", "source", "Windows rationale", "formatting"])(
    "refuses an invented %s even when it has a valid shape",
    (kind) => {
      const edited = structuredClone(review);
      if (kind === "graph") edited.review.featureGraphs[0].sha256 = "f".repeat(64);
      if (kind === "source") {
        edited.review.sourceSha = "f".repeat(40);
        for (const graph of edited.review.featureGraphs) graph.sourceSha = edited.review.sourceSha;
      }
      if (kind === "Windows rationale") edited.review.windowsEdges[0].reason = "invented approval";
      const text =
        kind === "formatting" ? lock.reviewedTransitionText + "\n" : JSON.stringify(edited);
      expect(() => proof({ reviewedTransitionText: text })).toThrow(/manifest digest differs/);
    },
  );
});
