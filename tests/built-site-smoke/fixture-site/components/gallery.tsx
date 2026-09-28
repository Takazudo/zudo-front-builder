"use client";

import { computed, signal } from "@takazudo/zfb/zudo-react";

import { labels } from "./gallery-registry";

/**
 * The glob-consuming island (issue #1404 / #1385 pt.1). It imports
 * `gallery-registry.ts`, which calls `import.meta.glob(...)`. If the emitted
 * client bundle still carried a raw `import.meta.glob(...)` call it would throw
 * a TypeError the instant this hydrates. So:
 *   - "Gallery items: 2" rendering proves the glob expanded at build time
 *     (both ./gallery/*.tsx modules were collected).
 *   - the button incrementing on click proves the island actually hydrated
 *     (the bundle loaded, ran, and wired the handler).
 */
export function Gallery() {
  const i = signal(0);
  const selected = computed(() => labels[i.value] ?? "none");
  return (
    <div>
      <p id="gallery-count">Gallery items: {labels.length}</p>
      <button
        type="button"
        id="gallery-next"
        on:click={() => {
          i.value = (i.value + 1) % labels.length;
        }}
      >
        Selected: {selected}
      </button>
    </div>
  );
}
