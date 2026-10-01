"use client";
import { signal, Show } from "@takazudo/zfb/zudo-react";
export function BlogMenu() {
  const open = signal(false);
  return (
    <nav>
      <button on:click={() => (open.value = !open.value)}>Menu</button>
      <Show when={open}>{() => <a href="/posts">Posts</a>}</Show>
    </nav>
  );
}
