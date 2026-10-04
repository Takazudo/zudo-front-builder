"use client";

import { signal } from "@takazudo/zfb/zudo-react";

export const InferredArrow = ({ label }: { label: string }) => <button>{label}</button>;

export const ExportedExpression = function InnerName({ label }: { label: string }) {
  return <button>{label}</button>;
};

const equalDisplayNameCount = signal(0);

export function EqualDisplayName({ label }: { label: string }) {
  return (
    <button
      id="equal-display-name"
      type="button"
      on:click={() => {
        equalDisplayNameCount.value += 1;
      }}
    >
      {label}: {equalDisplayNameCount}
    </button>
  );
}

EqualDisplayName.displayName = "EqualDisplayName";
