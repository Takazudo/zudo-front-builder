"use client";

import { signal } from "@takazudo/zfb/zudo-react";
import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";

export function DefaultPanel() {
  const count = signal(0);
  return jsx("button", {
    id: "default-panel",
    type: "button",
    "on:click": () => {
      count.value += 1;
    },
    children: ["Default panel: ", count],
  });
}
