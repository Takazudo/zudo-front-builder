import wasmModule from "../../wasm/answer.wasm";

export const prerender = false;

function answer(module: WebAssembly.Module) {
  const exported = new WebAssembly.Instance(module).exports.answer;
  if (typeof exported !== "function") {
    throw new Error("Wasm dev fixture must export answer()");
  }
  return exported();
}

export default function OgPage() {
  return (
    <html lang="en">
      <head>
        <title>Wasm dev SSR smoke</title>
      </head>
      <body>WASM_DEV_SSR_VALUE:{answer(wasmModule)}</body>
    </html>
  );
}
