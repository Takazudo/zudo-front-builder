"use client";

import note from "./counter-note.txt?raw";

export function Counter() {
  const value = typeof __X__ === "undefined" ? "unset" : __X__;
  return (
    <button type="button" data-build-define={value}>
      {note}
    </button>
  );
}
