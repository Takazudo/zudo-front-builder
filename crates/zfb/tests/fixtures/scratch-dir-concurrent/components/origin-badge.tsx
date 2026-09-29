"use client";

declare const __ORIGIN__: string;

// `originMark` keeps the substituted define visible as a property in the emitted island bundle.
export const config = { originMark: __ORIGIN__ };

export function OriginBadge() {
  return <span>ISLAND-ORIGIN-{config.originMark}</span>;
}
