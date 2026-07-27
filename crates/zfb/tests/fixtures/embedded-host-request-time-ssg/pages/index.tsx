// Build-time SSG denial case (#2019, epic #2012, guardrail 4): this
// project deliberately has NO `prerender = false` route (so `zfb build`
// needs no adapter configured — see the "Scope decision" note in
// crates/zfb/tests/preview_cross_mode_e2e.rs for the precedent: a
// project mixing an SSR-only route with no SSR-capable adapter hard-fails
// `zfb build`'s `ensure_no_ssr_without_adapter` check before this page's
// render is ever reached). This page (default `prerender`, i.e. SSG)
// calls `fetch()` at build time and re-throws its message wrapped in a
// marker the Rust test greps for in `zfb build`'s failure output, proving
// the deliberate, unchanged build-time network denial (guardrail 4)
// survives the whole epic's request-time work untouched.
export default async function IndexPage() {
  try {
    await fetch("http://127.0.0.1:1/unreachable");
  } catch (error) {
    throw new Error(`SSG_DENIAL_MARKER:${error.message}`);
  }
  return (
    <html lang="en">
      <body>SSG_UNEXPECTED_SUCCESS</body>
    </html>
  );
}
