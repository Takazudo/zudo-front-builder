"use client";

import { useState } from "preact/hooks";

export function Counter() {
  const [n, setN] = useState(0);
  return (
    <button type="button" onClick={() => setN(n + 1)}>
      Count: {n}
    </button>
  );
}
