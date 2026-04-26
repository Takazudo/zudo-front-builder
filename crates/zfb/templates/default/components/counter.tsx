"use client";

import { useState } from "preact/hooks";

type Props = {
  initial?: number;
};

export default function Counter({ initial = 0 }: Props) {
  const [count, setCount] = useState(initial);

  return (
    <button
      type="button"
      onClick={() => setCount((c) => c + 1)}
      class="counter"
    >
      Clicked {count} {count === 1 ? "time" : "times"}
    </button>
  );
}
