"use client";

import { signal } from "@takazudo/zfb/zudo-react";
import { jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { liveMessage } from "../helpers/live-message.js";

function PackedCounter({ label }) {
  const count = signal(0);
  return jsxs("button", {
    id: "packed-counter",
    type: "button",
    "on:click": () => {
      globalThis.__zfbPackedCounterClickCount =
        (globalThis.__zfbPackedCounterClickCount ?? 0) + 1;
      globalThis.__zfbPackedSignalSubscribers = count.subscribers.size;
      count.value += 1;
    },
    children: [label, ": ", liveMessage(), " ", count],
  });
}
PackedCounter.displayName = "PackedCounter";

export { PackedCounter, PackedCounter as default };
