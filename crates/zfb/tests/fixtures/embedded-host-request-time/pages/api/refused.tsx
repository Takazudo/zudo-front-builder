// Host-op failure case (#2019, epic #2012): `__REFUSED_PORT__` is a
// loopback port the Rust test bound and then immediately closed before
// booting `zfb dev`, so nothing is listening there — a deterministic,
// public-internet-free way to force the Rust transport op
// (`op_zfb_fetch`) itself to fail (ECONNREFUSED), surfacing as
// `FetchError::Transport` (crates/zfb-render/src/embedded_v8/fetch.rs).
// This is distinct from the resource-exhaustion case above: there the
// op succeeds and JS-visible policy rejects the 51st call; here the
// underlying transport op fails outright.
export const prerender = false;

export default async function RefusedPage() {
  const port = __REFUSED_PORT__;
  try {
    await fetch(`http://127.0.0.1:${port}/nope`);
    return (
      <html lang="en">
        <body>REFUSED_UNEXPECTED_SUCCESS</body>
      </html>
    );
  } catch (error) {
    return (
      <html lang="en">
        <body>REFUSED_ERROR:{error.message}</body>
      </html>
    );
  }
}
