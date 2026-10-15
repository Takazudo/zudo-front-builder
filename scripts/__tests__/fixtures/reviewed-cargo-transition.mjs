import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

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
    oldLockSource: fileURLToPath(oldPath),
    reviewedTransitionText,
  };
}
