export const prerender = false;

export default function Bytes() {
  return new Response(new Uint8Array([0, 255, 1, 254]), {
    headers: { "Content-Type": "application/octet-stream" },
  });
}
