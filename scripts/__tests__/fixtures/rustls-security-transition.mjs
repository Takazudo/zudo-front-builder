import { readFileSync } from "node:fs";

// Frozen lock bytes, not a native measurement or a live contract re-pin.
export function rustlsSecurityLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldText = readFileSync(new URL("rustls-security-4314.old.lock", directory), "utf8");
  const reviewedTransitionText = readFileSync(
    new URL("rustls-security-4314.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
  const newText = oldText
    .replace('name = "rustls"\nversion = "0.23.45"', 'name = "rustls"\nversion = "0.23.46"')
    .replace(
      "0d41d731c7d2f962d1ccc364cec258de3c0e93b38c2fb3ba97ac74513048d634",
      "48e13bd8c0e9365c43cfa5c9e8f9ad49d3c8444926c9aac819e0e4dc503c8fdf",
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
