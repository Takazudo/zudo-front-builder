"use client";

import { computed, signal } from "@takazudo/zfb/zudo-react";

export function ThemeToggle() {
  const dark = signal(false);
  const label = computed(() => (dark.value ? "light" : "dark"));
  return (
    <button
      type="button"
      on:click={() => {
        dark.value = !dark.value;
      }}
    >
      {label}
    </button>
  );
}
