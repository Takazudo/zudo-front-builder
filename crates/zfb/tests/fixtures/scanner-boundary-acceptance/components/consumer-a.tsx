"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export function readState() {
  return "unused helper from consumer A";
}

export function ConsumerA({ label }: { label: string }) {
  const count = signal(0);
  return (
    <button
      id="consumer-a"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      {label}: {count}
    </button>
  );
}
