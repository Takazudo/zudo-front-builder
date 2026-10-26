import { readFileSync } from "node:fs";

// Frozen lock bytes, not a native measurement or a live contract re-pin.
export function tokioSecurityLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldText = readFileSync(new URL("tokio-security-4569.old.lock", directory), "utf8");
  const reviewedTransitionText = readFileSync(
    new URL("tokio-security-4569.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
  const newText = oldText
    .replace('name = "tokio"\nversion = "1.52.3"', 'name = "tokio"\nversion = "1.54.1"')
    .replace(
      "8fc7f01b389ac15039e4dc9531aa973a135d7a4135281b12d7c1bc79fd57fffe",
      "b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3",
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
