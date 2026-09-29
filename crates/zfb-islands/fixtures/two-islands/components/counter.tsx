"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export function Counter() {
  const n = signal(0);
  return (
    <button
      type="button"
      on:click={() => {
        n.value += 1;
      }}
    >
      Count: {n}
    </button>
  );
}
