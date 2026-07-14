// Ambient module typing for Wasm files bundled by zfb.
//
// This source file intentionally has no imports or exports: the declaration
// must remain global rather than becoming a module augmentation.
declare module "*.wasm" {
  const wasmModule: WebAssembly.Module;
  export default wasmModule;
}
