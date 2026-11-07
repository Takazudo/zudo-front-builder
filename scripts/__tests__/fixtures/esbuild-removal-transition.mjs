import { readFileSync } from "node:fs";

// Frozen lock bytes, not a native measurement or a live contract re-pin.
export function esbuildRemovalLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldText = readFileSync(new URL("esbuild-removal-4635.old.lock", directory), "utf8");
  const newText = readFileSync(new URL("esbuild-removal-4635.new.lock", directory), "utf8");
  const reviewedTransitionText = readFileSync(
    new URL("esbuild-removal-4635.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
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
