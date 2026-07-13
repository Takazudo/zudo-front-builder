import "@takazudo/zfb";
import wasmModule from "../fixture.wasm";

type Expect<Condition extends true> = Condition;
type IsAny<Value> = 0 extends 1 & Value ? true : false;
type IsWasmModule<Value> =
  IsAny<Value> extends true
    ? false
    : [Value] extends [WebAssembly.Module]
      ? [WebAssembly.Module] extends [Value]
        ? true
        : false
      : false;

type ImportedWasmIsAWebAssemblyModule = Expect<IsWasmModule<typeof wasmModule>>;
