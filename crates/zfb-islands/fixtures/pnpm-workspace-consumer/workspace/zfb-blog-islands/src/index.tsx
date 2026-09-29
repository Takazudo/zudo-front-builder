"use client";

import { signal } from "@takazudo/zfb/zudo-react";
// Issue #1703, Guard (a) fixture extension: a bare package-name import of a
// WORKSPACE SIBLING (as opposed to the framework import above, a regular
// framework dependency) reached from inside this island. This is the exact
// shape Guard (a) detects — see the companion assertion in
// `pnpm_workspace_consumer_fixture_yields_workspace_package_islands`.
import { themeAttr } from "@takazudo/zfb-blog-shared";

export function Counter() {
  const n = signal(0);
  return (
    <button
      type="button"
      on:click={() => {
        n.value += 1;
      }}
    >
      Count: {n}
    </button>
  );
}

export function ThemeToggle() {
  return (
    <button type="button" data-attr={themeAttr}>
      Toggle theme
    </button>
  );
}
