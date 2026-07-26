// Headline case (#2019, epic #2012): a `prerender = false` route whose
// handler performs a real outbound request AND uses Web Crypto, dispatched
// through the embedded V8 host in REQUEST-TIME mode (`ssr_adapter.rs` is
// the only production call site that sets `DispatchMode::RequestTime`).
//
// `__LOOPBACK_PORT__` is replaced by the Rust test with the port of a
// deterministic loopback server it spawned itself (guardrail 3 of epic
// #2012 — never the public internet).
export const prerender = false;

export default async function HappyPage() {
  const port = __LOOPBACK_PORT__;
  const response = await fetch(`http://127.0.0.1:${port}/happy`);
  const text = await response.text();

  const randomBytes = new Uint8Array(16);
  crypto.getRandomValues(randomBytes);
  const allZero = randomBytes.every((byte) => byte === 0);

  const uuid = crypto.randomUUID();

  const digestBuffer = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode("zfb-e2e-happy-path"),
  );
  const digestHex = [...new Uint8Array(digestBuffer)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");

  // Built as ONE string, not several adjacent JSX `{expr}` text nodes —
  // JSX collapses inter-element whitespace unpredictably, and the Rust
  // test parses these markers by exact substring, not by rendered
  // layout.
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
