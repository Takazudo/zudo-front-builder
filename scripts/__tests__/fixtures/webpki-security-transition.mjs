import { readFileSync } from "node:fs";

// Frozen lock bytes, not a native measurement or a live contract re-pin.
export function webpkiSecurityLock() {
  const directory = new URL("../../../research/v3-island-size/cargo-transitions/", import.meta.url);
  const oldText = readFileSync(new URL("webpki-security-4314.old.lock", directory), "utf8");
  const reviewedTransitionText = readFileSync(
    new URL("webpki-security-4314.json", directory),
    "utf8",
  );
  const review = JSON.parse(reviewedTransitionText);
  const newText = oldText
    .replace(
      'name = "rustls-webpki"\nversion = "0.103.15"',
      'name = "rustls-webpki"\nversion = "0.103.16"',
    )
    .replace(
      "f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2",
      "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281",
    )
    .replace(
      'name = "webpki-roots"\nversion = "1.0.7"',
      'name = "webpki-roots"\nversion = "1.0.10"',
    )
    .replace(
      "52f5ee44c96cf55f1b349600768e3ece3a8f26010c05265ab73f945bb1a2eb9d",
      "1c1c8e20c8a38fc7d65abfe74eb7280e4aa667e8eec8bc84996d2874810748d9",
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
