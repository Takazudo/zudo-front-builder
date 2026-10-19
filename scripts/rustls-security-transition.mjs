import { createHash } from "node:crypto";

const sha256 = (value) => createHash("sha256").update(value).digest("hex");

// Owner-approved exact decision for #4314; caller-supplied JSON is not authority.
const DECISION_SHA256 = "fe689aca3a29b012ca7fb96a3aad3dc9fc507f117a18be501e379dae2b3f04ff";
const OLD_LOCK_SHA256 = "230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138";
const NEW_LOCK_SHA256 = "563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd";

export function proveRustlsSecurityTransition(reviewText, oldText, newText) {
  if (sha256(reviewText) !== DECISION_SHA256) {
    throw new Error("rustls security transition decision differs from the approved bytes");
  }
  if (sha256(oldText) !== OLD_LOCK_SHA256 || sha256(newText) !== NEW_LOCK_SHA256) {
    throw new Error("rustls security transition requires the exact approved old/new lock bytes");
  }
  const review = JSON.parse(reviewText);
  return {
    id: review.id,
    manifestSha256: DECISION_SHA256,
    sourceSha: review.sourceSha,
    added: review.added,
    changed: review.changed,
    removed: review.removed,
  };
}
