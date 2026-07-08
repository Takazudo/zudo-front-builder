/**
 * Ramp-native color schemes (zudo-doc v3).
 *
 * A `ColorScheme` is `{ ramps, map }`:
 *   - `ramps` is the shared Tier-1 source of truth.
 *   - `map` is the per-mode wiring from UI roles to ramp stops.
 */

import type { ColorScheme, ModeMap, Ramps } from "./color-scheme-utils";

export type { ColorScheme } from "./color-scheme-utils";

const ramps: Ramps = {
  base: [
    "oklch(.965 .004 65)",
    "oklch(.705 .008 65)",
    "oklch(.480 .008 65)",
    "oklch(.300 .006 65)",
    "oklch(.185 .005 65)",
  ],
  accent: ["oklch(.755 .130 64)", "oklch(.700 .158 62)", "oklch(.470 .120 56)"],
  state: {
    danger: "oklch(.640 .170 25)",
    success: "oklch(.680 .145 145)",
    warning: "oklch(.760 .135 82)",
    info: "oklch(.680 .130 245)",
  },
};

const darkMap: ModeMap = {
  bg: { base: 4 },
  fg: { base: 0 },
  selectionBg: { base: 2 },
  selectionFg: { base: 0 },
  semantic: {
    surface: { base: 4 },
    muted: { base: 1 },
    accent: { accent: 1 },
    accentHover: { accent: 0 },
    codeBg: { base: 3 },
    codeFg: { base: 0 },
    success: { state: "success" },
    danger: "oklch(.655 .170 25)",
    warning: { state: "warning" },
    info: { state: "info" },
    mermaidNodeBg: { base: 3 },
    mermaidText: { base: 0 },
    mermaidLine: { base: 1 },
    mermaidLabelBg: { base: 3 },
    mermaidNoteBg: { base: 2 },
    chatUserBg: { accent: 1 },
    chatUserText: { base: 4 },
    chatAssistantBg: { base: 4 },
    chatAssistantText: { base: 0 },
    imageOverlayBg: { base: 4 },
    imageOverlayFg: { base: 0 },
    matchedKeywordBg: "oklch(.700 .158 62)",
    matchedKeywordFg: "oklch(.300 .003 65)",
  },
};

const lightMap: ModeMap = {
  bg: { base: 0 },
  fg: { base: 4 },
  selectionBg: { base: 1 },
  selectionFg: { base: 4 },
  semantic: {
    surface: { base: 0 },
    muted: { base: 2 },
    accent: { accent: 2 },
    accentHover: "oklch(.400 .096 56)",
    codeBg: { base: 0 },
    codeFg: { base: 4 },
    success: "oklch(.470 .140 145)",
    danger: "oklch(.505 .170 25)",
    warning: "oklch(.490 .100 82)",
    info: "oklch(.485 .122 245)",
    mermaidNodeBg: { base: 1 },
    mermaidText: { base: 4 },
    mermaidLine: { base: 2 },
    mermaidLabelBg: { base: 1 },
    mermaidNoteBg: { base: 1 },
    chatUserBg: { accent: 1 },
    chatUserText: { base: 4 },
    chatAssistantBg: { base: 0 },
    chatAssistantText: { base: 4 },
    imageOverlayBg: { base: 4 },
    imageOverlayFg: { base: 0 },
    matchedKeywordBg: "oklch(.700 .158 62)",
    matchedKeywordFg: "oklch(.300 .003 65)",
  },
};

export const colorSchemes: Record<string, ColorScheme> = {
  "Default Light": { ramps, map: lightMap },
  "Default Dark": { ramps, map: darkMap },
};
