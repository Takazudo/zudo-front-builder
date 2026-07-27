// Resource-exhaustion case (#2019, epic #2012): the per-dispatch
// subrequest budget (`MAX_SUBREQUESTS_PER_DISPATCH = 50`,
// research/2013-request-time-capability-contract.md) is enforced in
// Rust, so a `Promise.all` fan-out from bundle code cannot evade it
// (crates/zfb-render/src/embedded_v8/mod.rs `begin_dispatch_subrequest_budget`).
// This handler issues 51 concurrent fetches against the SAME loopback
// server the happy-path route uses, one more than the budget allows.
export const prerender = false;

export default async function ExhaustPage() {
  const port = __LOOPBACK_PORT__;
  const calls = [];
  for (let i = 0; i < 51; i++) {
    calls.push(fetch(`http://127.0.0.1:${port}/happy`));
  }
  try {
    await Promise.all(calls);
    return (
      <html lang="en">
        <body>EXHAUST_UNEXPECTED_SUCCESS</body>
      </html>
    );
  } catch (error) {
    return (
      <html lang="en">
        <body>EXHAUST_ERROR:{error.message}</body>
      </html>
    );
  }
}
