import { createHash } from "node:crypto";

const sha256 = (value) => createHash("sha256").update(value).digest("hex");

// Each entry is one exact owner-approved replacement (the #4314 security fixes,
// the #4268 noyalib adoption): the decision bytes and both complete lock hashes
// are pinned here, so caller-supplied JSON is not authority. Adding an entry is
// a reviewed provenance decision, not a refactor.
function securityLockTransition({
  id,
  label,
  kind = "security",
  decisionSha256,
  oldLockSha256,
  newLockSha256,
}) {
  return Object.freeze({
    id,
    decisionSha256,
    oldLockSha256,
    newLockSha256,
    prove(reviewText, oldText, newText) {
      if (sha256(reviewText) !== decisionSha256) {
        throw new Error(`${label} ${kind} transition decision differs from the approved bytes`);
      }
      if (sha256(oldText) !== oldLockSha256 || sha256(newText) !== newLockSha256) {
        throw new Error(
          `${label} ${kind} transition requires the exact approved old/new lock bytes`,
        );
      }
      const review = JSON.parse(reviewText);
      return {
        id: review.id,
        manifestSha256: decisionSha256,
        sourceSha: review.sourceSha,
        added: review.added,
        changed: review.changed,
        removed: review.removed,
      };
    },
  });
}

export const RUSTLS_SECURITY_TRANSITION = securityLockTransition({
  id: "rustls-security-4314",
  label: "rustls",
  decisionSha256: "fe689aca3a29b012ca7fb96a3aad3dc9fc507f117a18be501e379dae2b3f04ff",
  oldLockSha256: "230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138",
  newLockSha256: "563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd",
});

export const WEBPKI_SECURITY_TRANSITION = securityLockTransition({
  id: "webpki-security-4314",
  label: "webpki",
  decisionSha256: "f7be894d377eb9224871eaefc73bf4fdb774318e144000e5d96145d090508021",
  oldLockSha256: "0cd091b7adcc7776895d9de119ac28ac8e8a931cdf28e46c676d5460627cc6f2",
  newLockSha256: "d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d",
});

export const NOYALIB_LOCK_TRANSITION = securityLockTransition({
  id: "noyalib-4268",
  label: "noyalib",
  kind: "dependency",
  decisionSha256: "5ff33c03ac56e9d494333153d7e5e2cf2e5bf4a2190f0b65fbe145b3c42e1994",
  oldLockSha256: "d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d",
  newLockSha256: "c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64",
});

export const SECURITY_LOCK_TRANSITIONS = Object.freeze([
  RUSTLS_SECURITY_TRANSITION,
  WEBPKI_SECURITY_TRANSITION,
  NOYALIB_LOCK_TRANSITION,
]);

export function securityLockTransitionById(id) {
  return SECURITY_LOCK_TRANSITIONS.find((entry) => entry.id === id) ?? null;
}
