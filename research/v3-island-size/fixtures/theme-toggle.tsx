"use client";
import { computed, signal } from "@takazudo/zfb/zudo-react";
export function ThemeToggle() {
  const dark = signal(false);
  const label = computed(() => (dark.value ? "Light" : "Dark"));
  return (
    <button aria-label="theme" on:click={() => (dark.value = !dark.value)}>
      {label}
    </button>
  );
}
