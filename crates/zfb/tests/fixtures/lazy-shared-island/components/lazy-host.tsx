"use client";

import { signal } from "@takazudo/zfb/zudo-react";
import { claimSharedState, sharedState } from "./shared-state";

claimSharedState("eager");

export function LazyHost({ label }: { label: string }) {
  const text = signal(`${label}: owner=${sharedState.owner}`);
  return (
    <button
      type="button"
      data-lazy-host=""
      on:click={() =>
        import("./lazy-panel").then(({ describeSharedState }) => {
          text.value = describeSharedState();
        })
      }
    >
      {text}
    </button>
  );
}
