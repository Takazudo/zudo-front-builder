"use client";

import { signal } from "@takazudo/zfb/zudo-react";
import { claimSharedState } from "./shared-state";

claimSharedState("eager");

export function Counter({ label }: { label: string }) {
  if (typeof document !== "undefined") {
    const scope = globalThis as { __ssrSharedAssetsMounts?: number };
    scope.__ssrSharedAssetsMounts = (scope.__ssrSharedAssetsMounts ?? 0) + 1;
  }
  const count = signal(0);
  const lazy = signal("lazy idle");
  return (
    <div>
      <button type="button" data-counter="" on:click={() => (count.value += 1)}>
        {label}: {count}
      </button>
      <button
        type="button"
        data-lazy=""
        on:click={() =>
          import("./lazy-panel").then(({ describeSharedState }) => {
            lazy.value = describeSharedState();
          })
        }
      >
        {lazy}
      </button>
    </div>
  );
}
