// Parity case 2 — divergence D7 (research/2013-request-time-capability-contract.md):
// `crypto.subtle.digest("MD5", ...)` is a documented legacy extension in
// real workerd, but the zfb embedded host deliberately does NOT implement
// it (only SHA-1/256/384/512). This is the SAME handler built for the
// Cloudflare adapter and served unmodified under `zfb dev`, so the two
// runtimes must diverge in EXACTLY the documented way: production
// succeeds with a real MD5 digest, the embedded host fails closed with
// `NotSupportedError`.
//
// The input "abc" is RFC 1321's own MD5 test vector
// (900150983cd24fb0d6963f7d28e17f72), so the Rust test can assert the
// production digest against a well-known value rather than trusting
// workerd's own output blindly.
export const prerender = false;

export default async function LegacyDigestPage() {
  try {
    const digestBuffer = await crypto.subtle.digest("MD5", new TextEncoder().encode("abc"));
    const digestHex = [...new Uint8Array(digestBuffer)]
      .map((byte) => byte.toString(16).padStart(2, "0"))
      .join("");
    return (
      <html lang="en">
        <body>{`LEGACY_DIGEST_OK:${digestHex}`}</body>
      </html>
    );
  } catch (error) {
    const message = `LEGACY_DIGEST_ERROR_NAME:${error.name}|LEGACY_DIGEST_ERROR_MESSAGE:${error.message}`;
    return (
      <html lang="en">
        <body>{message}</body>
      </html>
    );
  }
}
