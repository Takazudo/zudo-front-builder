/** Native ESM model shared by browser islands and Node starter generators. */
export type PresetId = "everyday" | "editorial" | "workbench";
export type HeadingFont = "sans" | "serif" | "mono";
export type PreviewMode = "page" | "components" | "foundations";
export type DesignRole = "accent" | "body" | "space";
export type WorkshopRoute = "start" | "playground" | "starter";
export interface DesignValues {
  accent: string;
  background: string;
  surface: string;
  ink: string;
  muted: string;
  border: string;
  headingFont: HeadingFont;
  bodySize: number;
  lineHeight: number;
  groupGap: number;
  sectionGap: number;
  horizontalSpace: number;
  horizontalGap: number;
  radius: number;
  readingWidth: number;
  headingSize: number;
}
export type NumericControl =
  | "bodySize"
  | "lineHeight"
  | "groupGap"
  | "sectionGap"
  | "horizontalSpace"
  | "radius"
  | "readingWidth";
export type EditableKey = "accent" | "headingFont" | NumericControl;
export interface WorkshopState {
  preset: PresetId;
  values: DesignValues;
  preview: PreviewMode;
  width: "full" | "mobile";
  inspect: boolean;
  role: DesignRole;
  notes: boolean;
  route: WorkshopRoute;
}
export interface WorkshopSnapshot extends WorkshopState {
  version: 1;
}
export interface DesignPreset {
  readonly name: string;
  readonly tag: string;
  readonly description: string;
  readonly detail: string;
  readonly traits: readonly string[];
  readonly values: Readonly<DesignValues>;
  readonly rules: readonly string[];
}
export type SeedFileName =
  | "zfb.design.ts"
  | "design-system.css"
  | "usage.tsx"
  | "DESIGN-NOTES.md"
  | "demo.html";
export type SeedFiles = Record<SeedFileName, string>;
/** Current Wind consumer shape; independent of the docs host's pinned renderer. */
export interface SeedWindConfig {
  spec: 1;
  reset: "owned-v1";
  tokens: {
    colors: Record<string, string>;
    spacing: Record<string, string>;
    sizes: Record<string, string>;
    fontSizes: Record<string, { size: string; lineHeight: string }>;
    fontFamilies: Record<string, string>;
    fontWeights: Record<string, string>;
    radii: Record<string, string>;
  };
  breakpoints: Record<string, { minWidthPx: number }>;
}
export const SNAPSHOT_VERSION: 1;
export const PRESETS: Readonly<Record<PresetId, DesignPreset>>;
export const FONTS: Readonly<Record<HeadingFont, string>>;
export const KEYS: readonly EditableKey[];
export const HEX: RegExp;
export const LIMITS: Readonly<Record<NumericControl, readonly [number, number]>>;
export function createState(preset?: PresetId): WorkshopState;
/** Supports versionless prototype snapshots, clamps finite numeric controls. */
export function restoreSnapshot(saved: unknown): WorkshopState;
export function createSnapshot(state: WorkshopState): WorkshopSnapshot;
export function isValidValue(key: unknown, value: unknown): boolean;
/** Invalid input preserves the previous valid value. */
export function setValue(state: WorkshopState, key: string, value: unknown): boolean;
export function luminance(hex: string): number;
export function contrast(a: string, b: string): number;
export function onAccent(values: Pick<DesignValues, "accent">): string;
export function variableMap(values: DesignValues): Record<string, string>;
export function variablesCSS(values: DesignValues): string;
export function currentRules(state: WorkshopState): string[];
export function pageMarkup(state: WorkshopState, notes?: boolean): string;
export function componentMarkup(): string;
export function foundationMarkup(state: WorkshopState): string;
/** Always accepts state explicitly; inspection is opt-in, disabled in standalone. */
export function previewDocument(
  state: WorkshopState,
  mode?: PreviewMode,
  notes?: boolean,
  standalone?: boolean,
  inspection?: boolean,
): string;
/** The vocabulary refers to CSS variables, so values live in variablesCSS. */
export function windObject(values?: DesignValues): SeedWindConfig;
/** The same generator supplies the export dialog, download, and drift checks. */
export function getSeedFiles(state: WorkshopState): SeedFiles;
export function crc32(bytes: Uint8Array): number;
/** Deterministic, uncompressed UTF-8 ZIP, with a fixed DOS date. */
export function zipData(files: Record<string, string>): Uint8Array;
/** Browser-compatible local download wrapper over zipData. */
export function zipBytes(files: Record<string, string>): Blob;
