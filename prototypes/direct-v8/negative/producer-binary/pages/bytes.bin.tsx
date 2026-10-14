export default function BytesPage() {
  return new Response(new Uint8Array([0, 1, 127, 128, 255]), {
    status: 200,
    headers: [
      ["content-type", "application/octet-stream"],
      ["set-cookie", "first=1; Path=/"],
      ["set-cookie", "second=2; Path=/"],
      ["x-fixture", "left"],
      ["x-fixture", "right"],
    ],
  });
}
