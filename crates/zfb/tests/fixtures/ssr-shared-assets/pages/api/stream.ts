export const prerender = false;

export default function Stream() {
  let release: () => void = () => undefined;
  const later = new Promise<void>((resolve) => {
    release = resolve;
  });
  (globalThis as { __ssrSharedAssetsRelease?: () => void }).__ssrSharedAssetsRelease = release;
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    async start(controller) {
      controller.enqueue(encoder.encode("first;"));
      await later;
      controller.enqueue(encoder.encode("second"));
      controller.close();
    },
  });
  return new Response(body, { headers: { "Content-Type": "text/html; charset=utf-8" } });
}
