"use client";

import { computed, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

type Theme = "light" | "dark";
const STORAGE_KEY = "basic-blog:theme";

function readPersistedTheme(): Theme | null {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    return saved === "light" || saved === "dark" ? saved : null;
  } catch {
    return null;
  }
}

function readSystemTheme(): Theme {
  if (typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export default function ThemeToggle() {
  // Server and browser begin with the same value so the island can adopt its HTML.
  const theme = signal<Theme>("light");
  const isDark = computed(() => theme.value === "dark");
  const nextLabel = computed(() => `Switch to ${isDark.value ? "light" : "dark"} theme`);
  const scope = getScope();

  // Read browser preferences only after the island's HTML is adopted.
  scope.onActivate(() => {
    theme.value = readPersistedTheme() ?? readSystemTheme();
  });
  // The scope stops this effect when the island is disposed.
  scope.effect(() => {
    const current = theme.value;
    document.documentElement.dataset["theme"] = current;
    try {
      window.localStorage.setItem(STORAGE_KEY, current);
    } catch {
      // Storage is optional; the current page still changes theme.
    }
  });

  return (
    <button
      type="button"
      class="flex size-8 cursor-pointer items-center justify-center rounded-full border border-neutral-200 text-neutral-600 transition-colors hover:border-neutral-300 hover:text-neutral-900 dark:border-neutral-800 dark:text-neutral-400 dark:hover:border-neutral-700 dark:hover:text-neutral-100"
      aria-pressed={isDark}
      aria-label={nextLabel}
      on:click={() => {
        theme.value = isDark.value ? "light" : "dark";
      }}
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        {Show({
          when: isDark,
          children: () => (
            <>
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
            </>
          ),
          fallback: () => <path d="M20 14.5A8.5 8.5 0 1 1 9.5 4a7 7 0 0 0 10.5 10.5z" />,
        })}
      </svg>
    </button>
  );
}
