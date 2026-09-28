"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export function Counter({ label }: { label: string }) {
  const count = signal(0);
  return (
    <button type="button" on:click={() => count.value++}>
      {label}: {count}
    </button>
  );
}

export function SkipCounter({ label }: { label: string }) {
  const count = signal(1);
  return (
    <button type="button" on:click={() => count.value++}>
      {label}: {count}
    </button>
  );
}
