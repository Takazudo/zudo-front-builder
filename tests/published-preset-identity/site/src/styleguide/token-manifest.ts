/**
 * Design-token manifest for configured UI tokens.
 *
 * GENERATED — do not hand-edit. Run `zudo-sg gen-token-manifest` after changing
 * either configured source file, then commit the regenerated output.
 * `zudo-sg gen-token-manifest --check` fails on drift.
 *
 * Source of truth: `src/styles/ui-tokens.css`,
 * parsed by the `zudo-sg gen-token-manifest` CLI command
 * (@takazudo/zudo-sg's src/cli/token-manifest/ui-token-manifest.ts). Only
 * `default` values are derived from the CSS; `group`/`step`/`unit`/
 * `control`/`options`/`pill` are presentation metadata with no CSS
 * equivalent and are configured in that module's SPECS tables.
 *
 * Covers: Color / Spacing / Font / Size tabs.
 * Does NOT include any --zd-* doc-chrome tokens.
 */
import type { TokenDef } from "@takazudo/zdtp";

// --- Font weight select options ---
const FONT_WEIGHT_OPTIONS = [
  "100", "200", "300", "400", "500", "600", "700", "800", "900",
] as const;

/**
 * Tier-1 raw palette colors from `src/styles/ui-tokens.css` (the `:root`
 * `--palette-{group}-{step-or-role}` block). This is the raw material beneath the
 * semantic `--color-*` tokens in UI_COLOR_TOKENS below — same three-tier
 * model the doc-chrome panel exposes via `--palette-*` ramps and `--zd-*`
 * semantic roles.
 *
 * These are plain name/value descriptors (NOT `TokenDef`) because zdtp's
 * `TokenDef.control` has no `"color"` option — the preview panel builds them
 * into `{ kind: "color" }` TierItems inline, mirroring the doc panel's
 * ramp tiers. Rendered as a "Palette" swatch tier in the preview
 * panel's Color tab; editing a swatch pushes `--palette-*` to the preview
 * iframes via the sink, cascading into every semantic token that references it.
 *
 * Coverage: 36 colors.
 */
export interface UiPaletteColor {
  /** Palette key without the `--palette-` prefix, e.g. "neutral-2". */
  name: string;
  /** Raw oklch value, from `src/styles/ui-tokens.css`. */
  value: string;
}

export const UI_PALETTE_COLORS: readonly UiPaletteColor[] = [
  { name: "neutral-0", value: "#ffffff" },
  { name: "neutral-1", value: "#e2e8f0" },
  { name: "neutral-2", value: "#475569" },
  { name: "neutral-3", value: "#0f172a" },
  { name: "accent-0", value: "#dbeafe" },
  { name: "accent-1", value: "#93c5fd" },
  { name: "accent-2", value: "#2563eb" },
  { name: "accent-3", value: "#1e40af" },
  { name: "state-danger", value: "#dc2626" },
  { name: "state-danger-dark", value: "#f87171" },
  { name: "state-success", value: "#15803d" },
  { name: "state-success-dark", value: "#4ade80" },
  { name: "state-warning", value: "#a16207" },
  { name: "state-warning-dark", value: "#facc15" },
  { name: "state-info", value: "#0369a1" },
  { name: "state-info-dark", value: "#38bdf8" },
  { name: "line-vacuum-accent", value: "#2563eb" },
  { name: "line-vacuum-accent-dark", value: "#93c5fd" },
  { name: "line-vacuum-hover", value: "#1e40af" },
  { name: "line-vacuum-hover-dark", value: "#dbeafe" },
  { name: "line-process-accent", value: "#2563eb" },
  { name: "line-process-accent-dark", value: "#93c5fd" },
  { name: "line-process-hover", value: "#1e40af" },
  { name: "line-process-hover-dark", value: "#dbeafe" },
  { name: "line-laser-accent", value: "#2563eb" },
  { name: "line-laser-accent-dark", value: "#93c5fd" },
  { name: "line-laser-hover", value: "#1e40af" },
  { name: "line-laser-hover-dark", value: "#dbeafe" },
  { name: "line-meeting-accent", value: "#2563eb" },
  { name: "line-meeting-accent-dark", value: "#93c5fd" },
  { name: "line-meeting-hover", value: "#1e40af" },
  { name: "line-meeting-hover-dark", value: "#dbeafe" },
  { name: "line-beauty-accent", value: "#2563eb" },
  { name: "line-beauty-accent-dark", value: "#93c5fd" },
  { name: "line-beauty-hover", value: "#1e40af" },
  { name: "line-beauty-hover-dark", value: "#dbeafe" },
];

/**
 * Color tokens from `src/styles/ui-tokens.css`.
 *
 * All values use light-dark() for dual-scheme support. Defaults here are
 * the full CSS declarations including both light and dark sides.
 * Stored as read-only text rows because light-dark() expressions cannot
 * be driven by a single-axis slider.
 *
 * Coverage: 21 tokens total.
 */
export const UI_COLOR_TOKENS: readonly TokenDef[] = [
  {
    id: "ui-color-bg",
    cssVar: "--color-bg",
    label: "color-bg",
    group: "surface",
    default: "#ffffff",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-surface",
    cssVar: "--color-surface",
    label: "color-surface",
    group: "surface",
    default: "light-dark(#ffffff, #0f172a)",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-surface-2",
    cssVar: "--color-surface-2",
    label: "color-surface-2",
    group: "surface",
    default: "light-dark(#e2e8f0, #475569)",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-border",
    cssVar: "--color-border",
    label: "color-border",
    group: "surface",
    default: "#475569",
    step: 1,
    unit: "",
    control: "text",
  },
  // Translucent frost scrim derived from --color-bg (SPA loading overlay).
  {
    id: "ui-color-loading-scrim",
    cssVar: "--color-loading-scrim",
    label: "color-loading-scrim",
    group: "surface",
    default: "#ffffff80",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-fg",
    cssVar: "--color-fg",
    label: "color-fg",
    group: "text",
    default: "#0f172a",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-muted",
    cssVar: "--color-muted",
    label: "color-muted",
    group: "text",
    default: "#475569",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-accent",
    cssVar: "--color-accent",
    label: "color-accent",
    group: "accent",
    default: "#2563eb",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-accent-hover",
    cssVar: "--color-accent-hover",
    label: "color-accent-hover",
    group: "accent",
    default: "#1e40af",
    step: 1,
    unit: "",
    control: "text",
  },
  // Foreground token for text/icons on filled accent/state surfaces (consumed via `text-on-accent`).
  {
    id: "ui-color-on-accent",
    cssVar: "--color-on-accent",
    label: "color-on-accent",
    group: "accent",
    default: "#ffffff",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-focus",
    cssVar: "--color-focus",
    label: "color-focus",
    group: "accent",
    default: "#2563eb",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-bg",
    cssVar: "--color-rail-bg",
    label: "color-rail-bg",
    group: "rail",
    default: "#0f172a",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-bg-strong",
    cssVar: "--color-rail-bg-strong",
    label: "color-rail-bg-strong",
    group: "rail",
    default: "#0f172a",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-fg",
    cssVar: "--color-rail-fg",
    label: "color-rail-fg",
    group: "rail",
    default: "#ffffff",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-muted",
    cssVar: "--color-rail-muted",
    label: "color-rail-muted",
    group: "rail",
    default: "#e2e8f0",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-border",
    cssVar: "--color-rail-border",
    label: "color-rail-border",
    group: "rail",
    default: "#475569",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-rail-hover-bg",
    cssVar: "--color-rail-hover-bg",
    label: "color-rail-hover-bg",
    group: "rail",
    default: "#475569",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-success",
    cssVar: "--color-success",
    label: "color-success",
    group: "state",
    default: "#15803d",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-danger",
    cssVar: "--color-danger",
    label: "color-danger",
    group: "state",
    default: "#dc2626",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-warning",
    cssVar: "--color-warning",
    label: "color-warning",
    group: "state",
    default: "#a16207",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-color-info",
    cssVar: "--color-info",
    label: "color-info",
    group: "state",
    default: "#0369a1",
    step: 1,
    unit: "",
    control: "text",
  },
];

/**
 * Spacing tokens from `src/styles/ui-tokens.css`.
 *
 * Coverage: 15 tokens total.
 */
export const UI_SPACING_TOKENS: readonly TokenDef[] = [
  {
    id: "ui-hsp-2xs",
    cssVar: "--spacing-hsp-2xs",
    label: "hsp-2xs",
    group: "hsp",
    default: "0.125rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-xs",
    cssVar: "--spacing-hsp-xs",
    label: "hsp-xs",
    group: "hsp",
    default: "0.25rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-sm",
    cssVar: "--spacing-hsp-sm",
    label: "hsp-sm",
    group: "hsp",
    default: "0.5rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-md",
    cssVar: "--spacing-hsp-md",
    label: "hsp-md",
    group: "hsp",
    default: "1rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-lg",
    cssVar: "--spacing-hsp-lg",
    label: "hsp-lg",
    group: "hsp",
    default: "1.5rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-xl",
    cssVar: "--spacing-hsp-xl",
    label: "hsp-xl",
    group: "hsp",
    default: "2rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-hsp-2xl",
    cssVar: "--spacing-hsp-2xl",
    label: "hsp-2xl",
    group: "hsp",
    default: "3rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-3xs",
    cssVar: "--spacing-vsp-3xs",
    label: "vsp-3xs",
    group: "vsp",
    default: "0.0625rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-2xs",
    cssVar: "--spacing-vsp-2xs",
    label: "vsp-2xs",
    group: "vsp",
    default: "0.125rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-xs",
    cssVar: "--spacing-vsp-xs",
    label: "vsp-xs",
    group: "vsp",
    default: "0.25rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-sm",
    cssVar: "--spacing-vsp-sm",
    label: "vsp-sm",
    group: "vsp",
    default: "0.5rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-md",
    cssVar: "--spacing-vsp-md",
    label: "vsp-md",
    group: "vsp",
    default: "1rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-lg",
    cssVar: "--spacing-vsp-lg",
    label: "vsp-lg",
    group: "vsp",
    default: "1.5rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-xl",
    cssVar: "--spacing-vsp-xl",
    label: "vsp-xl",
    group: "vsp",
    default: "2rem",
    step: 0.025,
    unit: "rem",
  },
  {
    id: "ui-vsp-2xl",
    cssVar: "--spacing-vsp-2xl",
    label: "vsp-2xl",
    group: "vsp",
    default: "3rem",
    step: 0.025,
    unit: "rem",
  },
];

/**
 * Font tokens from `src/styles/ui-tokens.css`.
 *
 * Coverage: 22 tokens total.
 */
export const UI_FONT_TOKENS: readonly TokenDef[] = [
  {
    id: "ui-text-xs",
    cssVar: "--text-xs",
    label: "text-xs",
    group: "font-size",
    default: "0.75rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-sm",
    cssVar: "--text-sm",
    label: "text-sm",
    group: "font-size",
    default: "0.875rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-base",
    cssVar: "--text-base",
    label: "text-base",
    group: "font-size",
    default: "1rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-lg",
    cssVar: "--text-lg",
    label: "text-lg",
    group: "font-size",
    default: "1.125rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-xl",
    cssVar: "--text-xl",
    label: "text-xl",
    group: "font-size",
    default: "1.25rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-2xl",
    cssVar: "--text-2xl",
    label: "text-2xl",
    group: "font-size",
    default: "1.5rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-text-xs--line-height",
    cssVar: "--text-xs--line-height",
    label: "text-xs / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-text-sm--line-height",
    cssVar: "--text-sm--line-height",
    label: "text-sm / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-text-base--line-height",
    cssVar: "--text-base--line-height",
    label: "text-base / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-text-lg--line-height",
    cssVar: "--text-lg--line-height",
    label: "text-lg / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-text-xl--line-height",
    cssVar: "--text-xl--line-height",
    label: "text-xl / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-text-2xl--line-height",
    cssVar: "--text-2xl--line-height",
    label: "text-2xl / lh",
    group: "font-size-lh",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-font-weight-normal",
    cssVar: "--font-weight-normal",
    label: "font-weight-normal",
    group: "font-weight",
    default: "400",
    step: 1,
    unit: "",
    control: "select",
    options: FONT_WEIGHT_OPTIONS,
  },
  {
    id: "ui-font-weight-medium",
    cssVar: "--font-weight-medium",
    label: "font-weight-medium",
    group: "font-weight",
    default: "500",
    step: 1,
    unit: "",
    control: "select",
    options: FONT_WEIGHT_OPTIONS,
  },
  {
    id: "ui-font-weight-semibold",
    cssVar: "--font-weight-semibold",
    label: "font-weight-semibold",
    group: "font-weight",
    default: "600",
    step: 1,
    unit: "",
    control: "select",
    options: FONT_WEIGHT_OPTIONS,
  },
  {
    id: "ui-font-weight-bold",
    cssVar: "--font-weight-bold",
    label: "font-weight-bold",
    group: "font-weight",
    default: "700",
    step: 1,
    unit: "",
    control: "select",
    options: FONT_WEIGHT_OPTIONS,
  },
  {
    id: "ui-leading-tight",
    cssVar: "--leading-tight",
    label: "leading-tight",
    group: "line-height",
    default: "1.25",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-leading-snug",
    cssVar: "--leading-snug",
    label: "leading-snug",
    group: "line-height",
    default: "1.375",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-leading-normal",
    cssVar: "--leading-normal",
    label: "leading-normal",
    group: "line-height",
    default: "1.5",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-leading-relaxed",
    cssVar: "--leading-relaxed",
    label: "leading-relaxed",
    group: "line-height",
    default: "1.625",
    step: 0.05,
    unit: "",
  },
  {
    id: "ui-font-sans",
    cssVar: "--font-sans",
    label: "font-sans",
    group: "font-family",
    default: "sans-serif",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-font-mono",
    cssVar: "--font-mono",
    label: "font-mono",
    group: "font-family",
    default: "monospace",
    step: 1,
    unit: "",
    control: "text",
  },
];

/**
 * Size tokens from `src/styles/ui-tokens.css`.
 *
 * Coverage: 8 tokens total.
 * `--radius-full` carries a pill toggle (sentinel 9999px).
 */
export const UI_SIZE_TOKENS: readonly TokenDef[] = [
  {
    id: "ui-radius-DEFAULT",
    cssVar: "--radius-DEFAULT",
    label: "radius-DEFAULT",
    group: "radius",
    default: "0.25rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-radius-sm",
    cssVar: "--radius-sm",
    label: "radius-sm",
    group: "radius",
    default: "0.25rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-radius-md",
    cssVar: "--radius-md",
    label: "radius-md",
    group: "radius",
    default: "0.5rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-radius-lg",
    cssVar: "--radius-lg",
    label: "radius-lg",
    group: "radius",
    default: "1rem",
    step: 0.05,
    unit: "rem",
  },
  {
    id: "ui-radius-full",
    cssVar: "--radius-full",
    label: "radius-full",
    group: "radius",
    default: "9999px",
    step: 1,
    unit: "px",
    pill: { value: "9999px", customDefault: "16px" },
  },
  {
    id: "ui-shadow-card",
    cssVar: "--shadow-card",
    label: "shadow-card",
    group: "shadow",
    default: "0 1px 2px #0000001a",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-shadow-raised",
    cssVar: "--shadow-raised",
    label: "shadow-raised",
    group: "shadow",
    default: "0 2px 4px #0000001a",
    step: 1,
    unit: "",
    control: "text",
  },
  {
    id: "ui-shadow-overlay",
    cssVar: "--shadow-overlay",
    label: "shadow-overlay",
    group: "shadow",
    default: "0 4px 8px #0000001a",
    step: 1,
    unit: "",
    control: "text",
  },
];
