"use client";

import { signal } from "zfb/zudo-react";

export default function AliasCounter() {
  const count = signal(0);
  return (
    <button
      id="mdx-alias-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Alias counter: {count}
    </button>
  );
}

export function AliasNamedCounter() {
  const count = signal(0);
  return (
    <button
      id="mdx-alias-named-counter"
      type="button"
      on:click={() => {
        count.value += 1;
      }}
    >
      Alias named counter: {count}
    </button>
  );
}
