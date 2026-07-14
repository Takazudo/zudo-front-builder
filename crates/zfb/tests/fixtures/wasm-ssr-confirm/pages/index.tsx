import ssgWasm from "../wasm/ssg-only.wasm";

function answer(module: WebAssembly.Module) {
  const exported = new WebAssembly.Instance(module).exports.answer;
  if (typeof exported !== "function") {
    throw new Error("SSG Wasm fixture must export answer()");
  }
  return exported();
}

export default function Page() {
  return (
    <html lang="en">
      <head>
        <title>Wasm SSR confirmation</title>
      </head>
      <body>
        <p>SSG_WASM_VALUE:{answer(ssgWasm)}</p>
      </body>
    </html>
  );
}
