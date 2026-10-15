"use client";
import { signal } from "zfb/zudo-react";
import wasmURL from "./answer.wasm";
import glueURL from "./glue.zfb-resource.mjs";
export function Counter() {
  const count = signal(0);
  return (
    <button
      id="counter"
      on:click={async () => {
        const { increment } = await import("./lazy");
        const worker = new Worker(new URL("./probe.ts", import.meta.url), { type: "module" });
        const workerResult = await new Promise<number>((resolve) => {
          worker.onmessage = (event) => resolve(event.data);
          worker.postMessage(40);
        });
        worker.terminate();
        const wasm = await WebAssembly.instantiate(
          await (await fetch(new URL(wasmURL, import.meta.url))).arrayBuffer(),
        );
        const glue = await import(/* @vite-ignore */ new URL(glueURL, import.meta.url).href);
        if (workerResult !== 42 || wasm.instance.exports.answer() !== 42 || glue.answer !== 42)
          throw new Error("companion failed");
        count.value += increment;
      }}
    >
      Count: {count}
    </button>
  );
}
