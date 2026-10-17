"use client";

import { signal } from "@takazudo/zfb/zudo-react";

// Same counter behavior as tests/built-site-smoke/fixture-site/components/counter.tsx.
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
