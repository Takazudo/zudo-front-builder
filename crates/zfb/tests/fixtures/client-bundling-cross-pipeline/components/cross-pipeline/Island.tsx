"use client";

import shader from "./demo.frag?raw";
import configuredLoader from "./entry.fixture";

declare const __CROSS_PIPELINE_DEFINE__: string;

const metaMode = import.meta.env.PROD ? "ZFB_ISLAND_META_PROD" : "ZFB_ISLAND_META_DEV";
const nodeMode =
  process.env.NODE_ENV === "production" ? "ZFB_ISLAND_NODE_PROD" : "ZFB_ISLAND_NODE_DEV";
const modeTuple = [
  "ZFB_ISLAND_ENTRY_MODE_TUPLE",
  import.meta.env.DEV,
  import.meta.env.PROD,
  process.env.NODE_ENV,
] as const;

export function CrossPipelineIsland() {
  if (import.meta.env.DEV) {
    console.info("ZFB_ISLAND_DEV_BRANCH");
  }

  return (
    <button
      id="cross-pipeline-island"
      data-contract={[
        shader,
        configuredLoader,
        __CROSS_PIPELINE_DEFINE__,
        metaMode,
        nodeMode,
        modeTuple,
      ].join("|")}
      onClick={() => new Worker(new URL("./worker.ts", import.meta.url), { type: "module" })}
    >
      ZFB_CROSS_PIPELINE_ISLAND
    </button>
  );
}
