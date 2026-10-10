import wasm from "../wasm/ssg-only.wasm";
export default function StaticOnly() {
  const answer = new WebAssembly.Instance(wasm).exports.answer as () => number;
  return (
    <html>
      <body>SSG_ONLY_318:{answer()}</body>
    </html>
  );
}
