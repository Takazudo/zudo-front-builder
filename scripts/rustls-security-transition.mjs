import { RUSTLS_SECURITY_TRANSITION } from "./security-lock-transitions.mjs";

// Owner-approved exact decision for #4314; caller-supplied JSON is not authority.
export const DECISION_SHA256 = RUSTLS_SECURITY_TRANSITION.decisionSha256;
export const OLD_LOCK_SHA256 = RUSTLS_SECURITY_TRANSITION.oldLockSha256;
export const NEW_LOCK_SHA256 = RUSTLS_SECURITY_TRANSITION.newLockSha256;

export function proveRustlsSecurityTransition(reviewText, oldText, newText) {
  return RUSTLS_SECURITY_TRANSITION.prove(reviewText, oldText, newText);
}
