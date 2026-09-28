"use client";

import { signal } from "@takazudo/zfb/zudo-react";

/**
 * The interactive island this whole smoke lane exists to exercise (issue
 * #1401). SSR renders "Count: 0" as static markup; a real click only
 * increments the count if the emitted client bundle actually loaded, ran,
 * and hydrated — which is exactly what the #1385 bug class breaks.
 */
export function Counter() {
  const count = signal(0);
  return (
    <button
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Count: {count}
    </button>
  );
}
