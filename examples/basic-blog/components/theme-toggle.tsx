"use client";

import { useEffect, useState } from "preact/hooks";

type Theme = "light" | "dark";

const STORAGE_KEY = "basic-blog:theme";

/**
 * Reads the persisted preference, falling back to the OS-level
 * `prefers-color-scheme` so the very first paint matches the user's system
 * setting. Safe to call during initial render: returns `"light"` on the
 * server (where there is no `window`), and the real value as soon as the
 * island hydrates.
 */
function readInitialTheme(): Theme {
  if (typeof window === "undefined") {
    return "light";
  }
  const saved = window.localStorage.getItem(STORAGE_KEY);
  if (saved === "light" || saved === "dark") {
    return saved;
  }
  const prefersDark =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches;
  return prefersDark ? "dark" : "light";
}

export default function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(readInitialTheme);

  useEffect(() => {
    document.documentElement.dataset["theme"] = theme;
    window.localStorage.setItem(STORAGE_KEY, theme);
  }, [theme]);

  const next: Theme = theme === "dark" ? "light" : "dark";
  return (
    <button
      type="button"
      class="theme-toggle"
      aria-label={`Switch to ${next} theme`}
      onClick={() => setTheme(next)}
    >
      {theme === "dark" ? "Light mode" : "Dark mode"}
    </button>
  );
}
