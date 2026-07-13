import ssrWasm from "../wasm/ssr-only.wasm";

export const prerender = false;

function answer(module: WebAssembly.Module) {
  const exported = new WebAssembly.Instance(module).exports.answer;
  if (typeof exported !== "function") {
    throw new Error("SSR Wasm fixture must export answer()");
  }
  return exported();
}

export default function RuntimePage() {
  return (
    <html lang="en">
      <head>
        <title>Wasm SSR runtime confirmation</title>
      </head>
      <body>
        <p>SSR_WASM_VALUE:{answer(ssrWasm)}</p>
      </body>
    </html>
  );
}
