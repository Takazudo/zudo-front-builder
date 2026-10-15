import { workerValue } from "./worker-dep.ts";

self.onmessage = () => {
  const nested = new Worker(new URL("./nested.ts", import.meta.url), { type: "module" });
  nested.onerror = (event) => {
    self.postMessage({ worker: workerValue, nested: `ERROR: ${event.message}` });
  };
  nested.onmessage = (event) => {
    self.postMessage({ worker: workerValue, nested: event.data });
    nested.terminate();
  };
  nested.postMessage("go");
};
