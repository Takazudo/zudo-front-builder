import wasm from "../wasm/runtime-only.wasm";
import common from "legacy-probe";
export const legacy = common.value;
export function answer() {
  return (new WebAssembly.Instance(wasm).exports.answer as () => number)();
}
