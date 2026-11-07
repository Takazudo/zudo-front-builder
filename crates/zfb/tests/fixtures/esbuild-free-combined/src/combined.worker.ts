// #4636: a module worker bundled as its own entry. The test executes the
// emitted worker under Node with a `self.postMessage` shim.
type WorkerMessage = readonly [marker: string, sum: number];

const message: WorkerMessage = ["ZFB_COMBINED_WORKER", [1, 2, 3].reduce((a, b) => a + b, 0)];

self.postMessage(message);
