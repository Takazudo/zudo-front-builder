"use client";

import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";

export function PreferredTarget({ label }) {
  return jsx("button", { children: label });
}
