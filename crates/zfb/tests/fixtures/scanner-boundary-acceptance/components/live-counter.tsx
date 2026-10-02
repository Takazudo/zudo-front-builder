"use client";

import { signal } from "@takazudo/zfb/zudo-react";
import { liveMessage } from "./helpers/live-message";

export function LiveCounter({ label }: { label: string }) {
  const count = signal(0);
  return (
    <button
      id="live-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      {label}: {liveMessage()} {count}
    </button>
  );
}
