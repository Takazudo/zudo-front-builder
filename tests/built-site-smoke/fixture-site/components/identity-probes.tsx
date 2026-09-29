"use client";

import { getScope, signal } from "@takazudo/zfb/zudo-react";
import { mountNewIslands } from "@takazudo/zfb/runtime";
import { counters } from "./proof-state";

export function IdentityProbe(props: { id: string }) {
  const left = signal(`left-${props.id}`);
  const empty = signal("");
  const right = signal(`right-${props.id}`);
  getScope().onActivate(() => {
    counters(props.id).mounts++;
    window.__proofActivateAgain = mountNewIslands;
    return () => {
      counters(props.id).cleanups++;
    };
  });
  return (
    <section id={`identity-${props.id}`}>
      <p id={`adjacent-${props.id}`}>
        lead{left}
        {empty}
        {right}tail
      </p>
      <>
        <strong id={`fragment-a-${props.id}`}>fragment A</strong>
        <em id={`fragment-b-${props.id}`}>fragment B</em>
      </>
      <button
        id={`identity-click-${props.id}`}
        on:click={() => {
          left.value += "!";
        }}
      >
        Change
      </button>
    </section>
  );
}

export function SkipSsrProbe() {
  getScope().onActivate(() => {
    counters("skip").mounts++;
  });
  return <span id="skip-client">client replacement</span>;
}

export function DeferredProbe() {
  getScope().onActivate(() => {
    counters("deferred").mounts++;
  });
  return <span id="deferred-client">deferred mounted</span>;
}
