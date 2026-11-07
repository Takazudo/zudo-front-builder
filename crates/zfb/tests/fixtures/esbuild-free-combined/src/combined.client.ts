// #4636: a client script that registers a module worker. Executed directly
// by the test under Node with `addEventListener` absent, it only logs.
const CLIENT_MARKER: string = "ZFB_COMBINED_CLIENT_SCRIPT";

console.info(CLIENT_MARKER);

if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-combined-worker",
    () => new Worker(new URL("./combined.worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
