import shader from "./demo.frag?raw";
import configuredLoader from "./entry.fixture";

declare const __CROSS_PIPELINE_DEFINE__: string;

const metaMode = import.meta.env.PROD ? "ZFB_CLIENT_META_PROD" : "ZFB_CLIENT_META_DEV";
const nodeMode =
  process.env.NODE_ENV === "production" ? "ZFB_CLIENT_NODE_PROD" : "ZFB_CLIENT_NODE_DEV";
const modeTuple = [
  "ZFB_CLIENT_ENTRY_MODE_TUPLE",
  import.meta.env.DEV,
  import.meta.env.PROD,
  process.env.NODE_ENV,
] as const;

if (import.meta.env.DEV) {
  console.info("ZFB_CLIENT_DEV_BRANCH");
}

console.info(
  "ZFB_CLIENT_ENTRY",
  shader,
  configuredLoader,
  __CROSS_PIPELINE_DEFINE__,
  metaMode,
  nodeMode,
  modeTuple,
);

// Client entries also pass through the SSR bundle loader. Keep browser-only
// registration lazy and capability-guarded so importing this module in V8
// never touches a missing DOM API or constructs the Worker.
if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-cross-pipeline-worker",
    () => new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
