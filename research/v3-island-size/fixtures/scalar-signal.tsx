"use client";
import { signal } from "@takazudo/zfb/zudo-react";
export function ScalarSignal() {
  const count = signal(0);
  return <button on:click={() => count.value++}>Count: {count}</button>;
}
