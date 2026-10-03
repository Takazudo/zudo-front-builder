"use client";

export function readState() {
  return "unused helper from consumer B";
}

export function ConsumerB({ label }: { label: string }) {
  return (
    <button id="consumer-b" type="button">
      {label}
    </button>
  );
}
