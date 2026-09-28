"use client";

import { getScope, signal } from "@takazudo/zfb/zudo-react";
import { counters } from "./proof-state";

function probe(name: string, label: string, ticking: boolean) {
  const count = signal(0);
  getScope().onActivate(() => {
    const state = counters(name);
    state.mounts++;
    const timer = ticking
      ? window.setInterval(() => {
          state.ticks++;
        }, 20)
      : undefined;
    return () => {
      if (timer !== undefined) window.clearInterval(timer);
      state.cleanups++;
    };
  });
  return (
    <button
      id={`${name}-button`}
      on:click={() => {
        count.value++;
      }}
    >
      {label}: {count}
    </button>
  );
}

export function DisposableProbe() {
  return probe("disposable", "Disposable", true);
}
export function EqualProbe(props: { label: string }) {
  return probe("equal", props.label, false);
}
export function ChangedProbe(props: { label: string }) {
  return probe("changed", props.label, false);
}
