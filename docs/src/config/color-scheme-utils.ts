export {
  STATE_ROLES,
  SEMANTIC_CSS_NAMES,
  SEMANTIC_KEYS,
  SEMANTIC_RAMP_DEFAULTS,
  generateCssCustomProperties as generateCssCustomPropertiesFromScheme,
  generateLightDarkCssProperties as generateLightDarkCssPropertiesFromSchemes,
  resolveRampRef,
  resolveSemanticColors,
  schemeToCssPairs,
  type ColorScheme,
  type CssEmitScope,
  type ModeMap,
  type OKLCH,
  type RampRef,
  type Ramps,
  type SemanticKey,
  type StateRole,
} from "@takazudo/zudo-doc/color-scheme-utils";
import {
  generateCssCustomProperties as _generateCssCustomProperties,
  generateLightDarkCssProperties as _generateLightDarkCssProperties,
} from "@takazudo/zudo-doc/color-scheme-utils";

import { colorSchemes } from "./color-schemes";
import { settings } from "./settings";

export const lightDarkPairings = [
  { light: "Default Light", dark: "Default Dark", label: "Default" },
];

export function getActiveScheme() {
  const scheme = colorSchemes[settings.colorScheme];
  if (!scheme) {
    throw new Error(
      `Unknown color scheme: "${settings.colorScheme}". Available: ${Object.keys(colorSchemes).join(", ")}`,
    );
  }
  return scheme;
}

export function generateCssCustomProperties(): string {
  return _generateCssCustomProperties(getActiveScheme());
}

export function generateLightDarkCssProperties(): string {
  if (!settings.colorMode) {
    throw new Error("colorMode is not configured");
  }
  const { lightScheme, darkScheme } = settings.colorMode;
  const light = colorSchemes[lightScheme];
  const dark = colorSchemes[darkScheme];
  if (!light) throw new Error(`Unknown light scheme: "${lightScheme}"`);
  if (!dark) throw new Error(`Unknown dark scheme: "${darkScheme}"`);
  return _generateLightDarkCssProperties(light, dark);
}
