"use client";
import { signal, Show } from "@takazudo/zfb/zudo-react";
export function JsonApi() {
  const result = signal("idle");
  async function load() {
    const response = await fetch("/api/status");
    result.value = JSON.stringify(await response.json());
  }
  return (
    <section>
      <button on:click={load}>Load</button>
      <Show when={signal(true)}>{() => <pre>{result}</pre>}</Show>
    </section>
  );
}
