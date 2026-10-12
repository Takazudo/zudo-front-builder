"use client";

// Real `"use client"` island reached through the generated registry. It is
// intentionally not statically imported by a host `pages/` file.

import { signal } from "@takazudo/zfb/zudo-react";

export default function Counter(props: { start?: number }) {
  const count = signal(props.start ?? 0);
  return (
    <button
      type="button"
      data-counter
      on:click={() => {
        count.value += 1;
      }}
    >
      count: {count}
    </button>
  );
}

Counter.displayName = "Counter";
