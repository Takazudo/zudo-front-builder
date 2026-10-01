"use client";
export function EventOnly() {
  return <button on:click={() => document.body.toggleAttribute("data-event")}>Toggle</button>;
}
