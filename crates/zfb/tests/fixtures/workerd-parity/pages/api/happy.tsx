// Parity case 1 (issue #2020, epic #2012, Wave 8): every contract row
// marked "supported" must behave identically whether this handler runs
// under `zfb dev`'s embedded V8 host (request-time SSR,
// research/2013-request-time-capability-contract.md) or under real
// Cloudflare Workers (`wrangler dev`, real workerd). This exact page
// source is built once for the Cloudflare adapter and served unmodified
// under `zfb dev` to make the comparison a same-handler comparison, not
// a same-behavior-by-convention one.
//
// `__LOOPBACK_PORT__` is substituted by the Rust test with the port of a
// deterministic loopback server it spawns itself (guardrail 3 of epic
// #2012 — never the public internet; real workerd's local dev mode can
// reach host loopback sockets just like the embedded host can).
export const prerender = false;

export default async function HappyPage() {
  const port = __LOOPBACK_PORT__;
  const response = await fetch(`http://127.0.0.1:${port}/happy`);
  const text = await response.text();

  const randomBytes = new Uint8Array(16);
  crypto.getRandomValues(randomBytes);
  const allZero = randomBytes.every((byte) => byte === 0);

  const uuid = crypto.randomUUID();

  // A fixed input string, so the Rust test can assert the SAME digest
  // hex reaches both runtimes byte-for-byte — the strongest possible
  // "identical observable behaviour" check for a supported contract row.
  const digestBuffer = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode("zfb-e2e-happy-path"),
  );
  const digestHex = [...new Uint8Array(digestBuffer)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");

  // One joined string / one JSX text node so inter-element whitespace
  // collapsing can never split a marker (same convention as #2019's
  // fixture).
  const message = [
    `HAPPY_FETCH_BODY:${text}`,
    `HAPPY_FETCH_STATUS:${response.status}`,
    `HAPPY_RANDOM_NONZERO:${String(!allZero)}`,
    `HAPPY_UUID:${uuid}`,
    `HAPPY_DIGEST:${digestHex}`,
  ].join("|");

  return (
    <html lang="en">
      <head>
        <title>happy</title>
      </head>
      <body>{message}</body>
    </html>
  );
}
