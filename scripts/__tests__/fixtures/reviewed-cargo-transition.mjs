import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

// Real reviewed lock bytes, synthetic measurement fixtures. Nothing here is a
// native platform measurement or permission to rewrite a live size contract.
// Both locks are frozen historical bytes, never the candidate's live Cargo.lock:
// admitting a later candidate lock is a separate, live-contract decision.
export function reviewedCargoLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldPath = new URL("rolldown-native-318.old.lock", directory);
  const newPath = new URL("rolldown-native-318.new.lock", directory);
  const reviewedTransitionText = readFileSync(
    new URL("rolldown-native-318.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
  const newText = readFileSync(newPath, "utf8");
  const newSha = createHash("sha256").update(newText).digest("hex");
  if (newSha !== review.newSha)
    throw new Error(
      `frozen reviewed new lock hashes to ${newSha}, not the review's ${review.newSha}`,
    );
  return {
    expectedOldSha: review.oldSha,
    expectedNewSha: review.newSha,
    oldText: readFileSync(oldPath, "utf8"),
    newText,
    added: review.delta.added.map((row) => row.id),
    // Verified historical fixture references, kept literal so shallow CI does not
    // need the historical commits. Production CLI provenance still resolves git.
    oldLockSource: {
      commit: "2c988535e7c4dca5053941e430e390290c6b9396",
      path: "research/v3-island-size/cargo-transitions/rolldown-native-318.old.lock",
      sha256: "2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e",
    },
    // The reviewed integration source named in cargo-transitions/README.md;
    // rolldown-native-318.new.lock is that commit's Cargo.lock, byte for byte.
    newLockSource: {
      commit: "adb507f7d3ed2de412c603f63cec6e38424dcadb",
      path: "Cargo.lock",
      sha256: "13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37",
    },
    reviewedTransitionText,
  };
}
