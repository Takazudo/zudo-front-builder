"use client";
import { For, Show, signal } from "@takazudo/zfb/zudo-react";
export function ShowFor() {
  const visible = signal(true);
  const items = signal(["one", "two"]);
  return (
    <div>
      <button on:click={() => (visible.value = !visible.value)}>Toggle</button>
      <Show when={visible}>
        {() => (
          <For each={items} by={(item) => item}>
            {(item) => <span>{item}</span>}
          </For>
        )}
      </Show>
    </div>
  );
}
