"use client";

import { useState } from "preact/hooks";

/**
 * The interactive island this whole smoke lane exists to exercise (issue
 * #1401). SSR renders "Count: 0" as static markup; a real click only
 * increments the count if the emitted client bundle actually loaded, ran,
 * and hydrated — which is exactly what the #1385 bug class breaks.
 */
export function Counter() {
  const [count, setCount] = useState(0);
  return (
    <button type="button" onClick={() => setCount(count + 1)}>
      Count: {count}
    </button>
  );
}
