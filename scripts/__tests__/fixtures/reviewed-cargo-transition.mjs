import { readFileSync } from "node:fs";

// Real reviewed lock bytes, synthetic measurement fixtures. Nothing here is a
// native platform measurement or permission to rewrite a live size contract.
export function reviewedCargoLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldPath = new URL("rolldown-native-318.old.lock", directory);
  const reviewedTransitionText = readFileSync(
    new URL("rolldown-native-318.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
  return {
    expectedOldSha: review.oldSha,
    expectedNewSha: review.newSha,
    oldText: readFileSync(oldPath, "utf8"),
    newText: readFileSync(new URL("../../../Cargo.lock", import.meta.url), "utf8"),
    added: review.delta.added.map((row) => row.id),
    // Verified historical fixture reference, kept literal so shallow CI does not
    // need the historical commit. Production CLI provenance still resolves git.
    oldLockSource: {
      commit: "2c988535e7c4dca5053941e430e390290c6b9396",
      path: "research/v3-island-size/cargo-transitions/rolldown-native-318.old.lock",
      sha256: "2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e",
    },
    reviewedTransitionText,
  };
}
