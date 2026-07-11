import rawPayload from "./worker.frag?raw";
import configuredLoader from "./worker.fixture";

declare const __CROSS_PIPELINE_DEFINE__: string;

if (import.meta.env.DEV) {
  self.postMessage("ZFB_ISLAND_WORKER_DEV_BRANCH");
}

const metaMode = import.meta.env.PROD
  ? "ZFB_ISLAND_WORKER_META_PROD"
  : "ZFB_ISLAND_WORKER_META_DEV";
const nodeMode =
  process.env.NODE_ENV === "production"
    ? "ZFB_ISLAND_WORKER_NODE_PROD"
    : "ZFB_ISLAND_WORKER_NODE_DEV";

new Worker(new URL("./nested.ts", import.meta.url), { type: "module" });
self.postMessage([
  "ZFB_ISLAND_WORKER_ENTRY",
  rawPayload,
  configuredLoader,
  __CROSS_PIPELINE_DEFINE__,
  metaMode,
  nodeMode,
]);
