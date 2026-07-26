// Unsupported-capability case (#2019, epic #2012): `crypto.subtle.encrypt`
// is a key-bearing SubtleCrypto method that fails closed on every host
// (divergence D8, research/2013-request-time-capability-contract.md). The
// point of this case is the DIAGNOSTIC, not the failure itself: before
// mode plumbing (#2014) landed, any capability gap reached through
// request-time SSR risked surfacing the build-time-only "fetch() called
// from SSG runtime" wording, which is the exact defect epic #2012 fixes.
// The assertion in the Rust test is that the message here names the
// "zfb embedded runtime" and never the SSG wording.
export const prerender = false;

export default async function UnsupportedPage() {
  try {
    await crypto.subtle.encrypt({ name: "AES-GCM" }, {}, new Uint8Array(1));
    return (
      <html lang="en">
        <body>UNSUPPORTED_UNEXPECTED_SUCCESS</body>
      </html>
    );
  } catch (error) {
    // Joined into one string / one JSX text node, not two adjacent
    // `{expr}`s, so the Rust test's substring checks never depend on
    // JSX's inter-element whitespace collapsing.
    const message = `UNSUPPORTED_ERROR_NAME:${error.name}|UNSUPPORTED_ERROR_MESSAGE:${error.message}`;
    return (
      <html lang="en">
        <body>{message}</body>
      </html>
    );
  }
}
