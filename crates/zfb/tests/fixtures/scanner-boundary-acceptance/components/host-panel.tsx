"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export function HostPanel() {
  const count = signal(0);
  return (
    <button
      id="host-panel"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Host panel: {count}
    </button>
  );
}
