// Parity case 3 — divergence D8 (research/2013-request-time-capability-contract.md):
// key-bearing SubtleCrypto (`generateKey`, `encrypt`, `decrypt`, ...) fails
// closed on the zfb embedded host; real workerd implements the full
// matrix. This handler performs a genuine AES-GCM generate-key -> encrypt
// -> decrypt round trip — not just "does it throw", but "does the
// plaintext survive a real round trip" — so a production PASS here is
// proof of real support, not an accidental non-throw. Built once for the
// Cloudflare adapter and served unmodified under `zfb dev`.
export const prerender = false;

const PLAINTEXT = "zfb-e2e-workerd-parity-plaintext";

export default async function KeyedCryptoPage() {
  try {
    const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, true, [
      "encrypt",
      "decrypt",
    ]);
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const plaintextBytes = new TextEncoder().encode(PLAINTEXT);
    const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, plaintextBytes);
    const decryptedBytes = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ciphertext);
    const decryptedText = new TextDecoder().decode(decryptedBytes);
    return (
      <html lang="en">
        <body>{`KEYED_CRYPTO_OK:${decryptedText}`}</body>
      </html>
    );
  } catch (error) {
    const message = `KEYED_CRYPTO_ERROR_NAME:${error.name}|KEYED_CRYPTO_ERROR_MESSAGE:${error.message}`;
    return (
      <html lang="en">
        <body>{message}</body>
      </html>
    );
  }
}
