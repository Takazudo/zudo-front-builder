import { Island as Boundary } from "@takazudo/zfb";
import { Fragment, jsx, jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { jsxDEV } from "@takazudo/zfb/zudo-react/jsx-dev-runtime";
import { PackedCounter } from "./barrels/cycle-a.js";
import { DefaultPanel } from "./client/default-panel.js";

export function PanelSlot({ children }) {
  return jsx(Fragment, { children });
}

export function DefaultPanelIsland() {
  return jsx(Fragment, {
    children: jsx(Boundary, {
      when: "load",
      children: jsx(DefaultPanel, {}),
    }),
  });
}

export function PackedForwardBoundary({ children }) {
  return jsx(Boundary, { children });
}

export function PackedOuterForwardBoundary({ children }) {
  return jsx(PackedForwardBoundary, { children });
}

export function PackedFixedBoundary() {
  return jsx(Boundary, { children: jsx(PackedCounter, { label: "Packed fixed" }) });
}

export function PackedJsxsBoundary() {
  return jsxs(Boundary, { children: [jsx(PackedCounter, { label: "Packed jsxs" })] });
}

export function PackedDevBoundary() {
  return jsxDEV(
    Boundary,
    { children: jsxDEV(PackedCounter, { label: "Packed jsxDEV" }, undefined, false, null, null) },
    undefined,
    false,
    null,
    null,
  );
}
