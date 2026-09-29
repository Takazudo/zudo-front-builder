"use client";
import { sharedCount } from "./shared-signal";

export function SharedWriter() {
  return (
    <button
      id="shared-writer"
      on:click={() => {
        sharedCount.value++;
      }}
    >
      Write: {sharedCount}
    </button>
  );
}
