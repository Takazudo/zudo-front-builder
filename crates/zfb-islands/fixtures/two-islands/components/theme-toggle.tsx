"use client";

import { useState } from "preact/hooks";

export function ThemeToggle() {
  const [dark, setDark] = useState(false);
  return (
    <button type="button" onClick={() => setDark(!dark)}>
      {dark ? "light" : "dark"}
    </button>
  );
}
