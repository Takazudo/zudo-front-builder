"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export function FactoryCounter() {
  const count = signal(0);
  return (
    <button
      id="factory-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Factory counter: {count}
    </button>
  );
}
