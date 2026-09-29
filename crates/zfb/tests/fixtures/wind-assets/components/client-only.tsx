"use client";

import { Show, signal } from "@takazudo/zfb/zudo-react";

export default function ClientOnly() {
  const open = signal(false);
  return (
    <section>
      <button
        type="button"
        on:click={() => {
          open.value = !open.value;
        }}
      >
        Toggle client-only class
      </button>
      {Show({
        when: open,
        children: () => <div class="bg-client-only">The client-only branch is open.</div>,
      })}
    </section>
  );
}
