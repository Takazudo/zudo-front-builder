import wasm from "../../../wasm/catchall-only.wasm";
import legacy from "legacy-probe";
export const prerender = false;
export default function CatchAll({ params }: { params: Record<string, string> }) {
  const answer = new WebAssembly.Instance(wasm).exports.answer as () => number;
  return new Response(`CATCH_318:${params.adminPath}:${answer()}:${legacy.value}`);
}
