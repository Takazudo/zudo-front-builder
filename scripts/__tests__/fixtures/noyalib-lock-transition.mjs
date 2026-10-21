import { readFileSync } from "node:fs";

// Frozen lock bytes, not a native measurement or a live contract re-pin.
export function noyalibLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldText = readFileSync(new URL("noyalib-4268.old.lock", directory), "utf8");
  const reviewedTransitionText = readFileSync(new URL("noyalib-4268.json", directory), "utf8");
  const review = JSON.parse(reviewedTransitionText);
  const newText = oldText
    .replace('name = "noyalib"\nversion = "0.0.55"', 'name = "noyalib"\nversion = "0.0.58"')
    .replace(
      "0c3ccce0306ea4477a655985ff2bf3e6c38f78f3e1defef6f30c7eb24d58cf41",
      "3fad8f1a552070f6ba5f60f25e880f7b6daeda8fe138f589345141d31a21a3bb",
    )
    .replace(
      'name = "noyalib-serde-yaml"\nversion = "0.0.55"',
      'name = "noyalib-serde-yaml"\nversion = "0.0.58"',
    )
    .replace(
      "4775f0afd3f1d5e55b3779e8e4b7ffde09d690629f6731e4047159a751c17a3b",
      "c7ef9e69591faecd546d11575c1f65c4c5f6725692cd82c6280eae4c1a923696",
    );
  return {
    oldText,
    newText,
    reviewedTransitionText,
    expectedOldSha: review.oldSha,
    expectedNewSha: review.newSha,
    added: review.added,
    oldLockSource: { commit: review.sourceSha, path: "Cargo.lock", sha256: review.oldSha },
  };
}
