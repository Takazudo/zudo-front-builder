import configuredLoader from "./nested.fixture";

declare const __CROSS_PIPELINE_DEFINE__: string;

const metaMode = import.meta.env.PROD
  ? "ZFB_CLIENT_NESTED_META_PROD"
  : "ZFB_CLIENT_NESTED_META_DEV";
const nodeMode =
  process.env.NODE_ENV === "production"
    ? "ZFB_CLIENT_NESTED_NODE_PROD"
    : "ZFB_CLIENT_NESTED_NODE_DEV";
const modeTuple = [
  "ZFB_CLIENT_NESTED_MODE_TUPLE",
  import.meta.env.DEV,
  import.meta.env.PROD,
  process.env.NODE_ENV,
] as const;

self.postMessage([
  "ZFB_CLIENT_NESTED_WORKER",
  configuredLoader,
  __CROSS_PIPELINE_DEFINE__,
  metaMode,
  nodeMode,
  modeTuple,
]);
