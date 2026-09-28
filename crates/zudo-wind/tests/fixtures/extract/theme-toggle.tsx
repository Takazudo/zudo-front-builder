"use client";

import { useEffect, useState } from "preact/hooks";

type Theme = "light" | "dark";

const STORAGE_KEY = "basic-blog:theme";

/**
 * Theme toggle island.
 *
 * SSR contract: the very first render — both on the server and during the
 * client hydration pass — MUST return identical HTML. Reading
 * `localStorage` / `matchMedia` from a `useState` initialiser breaks that
 * contract: the server has neither, so it defaults to `"light"`, but the
 * client reads the user's saved preference and immediately renders the
 * opposite label. That mismatch produces a hydration warning and a brief
 * flash of the wrong toggle.
 *
 * Fix: render a deterministic default (`"light"`) on first paint, then
 * sync to the persisted/system preference inside `useEffect`. To avoid a
 * FOUC of the wrong colour scheme on the surrounding page, an inline
 * pre-hydration script in `<head>` (see `layouts/default.tsx`) sets
 * `document.documentElement.dataset.theme` synchronously, BEFORE the
 * stylesheet is parsed, so the page paints in the correct theme on the
 * very first frame regardless of which label this island renders.
 */
function readPersistedTheme(): Theme | null {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (saved === "light" || saved === "dark") {
      return saved;
    }
  } catch {
    // Private browsing / disabled storage: fall through to system default.
  }
  return null;
}

function readSystemTheme(): Theme {
  if (typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export default function ThemeToggle() {
  // Deterministic SSR-safe default. Real preference is applied below.
  const [theme, setTheme] = useState<Theme>("light");

  // Sync once on mount: read persisted (or system) preference and apply it.
  useEffect(() => {
    const initial = readPersistedTheme() ?? readSystemTheme();
    setTheme(initial);
  }, []);

  // Mirror the active theme to the document and persist on change.
  useEffect(() => {
    document.documentElement.dataset["theme"] = theme;
    try {
      window.localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Storage unavailable; persistence is best-effort.
    }
  }, [theme]);

  const isDark = theme === "dark";
  const next: Theme = isDark ? "light" : "dark";
  return (
    <button
      type="button"
      class="flex size-8 cursor-pointer items-center justify-center rounded-full border border-neutral-200 text-neutral-600 transition-colors hover:border-neutral-300 hover:text-neutral-900 dark:border-neutral-800 dark:text-neutral-400 dark:hover:border-neutral-700 dark:hover:text-neutral-100"
      aria-pressed={isDark}
      aria-label={`Switch to ${next} theme`}
      onClick={() => setTheme(next)}
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
        {isDark ? (
          <>
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
          </>
        ) : (
          <path d="M20 14.5A8.5 8.5 0 1 1 9.5 4a7 7 0 0 0 10.5 10.5z" />
        )}
      </svg>
    </button>
  );
}
