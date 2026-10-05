"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export default function Counter() {
  const count = signal(0);
  return (
    <button
      id="mdx-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Counter: {count}
    </button>
  );
}

export function NamedCounter() {
  const count = signal(0);
  return (
    <button
      id="mdx-named-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Named counter: {count}
    </button>
  );
}
