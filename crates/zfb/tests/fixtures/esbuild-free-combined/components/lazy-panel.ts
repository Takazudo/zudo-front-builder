import { sharedState } from "./shared-state";

export const LAZY_PANEL_MARKER = "ZFB_LAZY_PANEL_CHUNK";

export function describeSharedState(): string {
  return `${LAZY_PANEL_MARKER} owner=${sharedState.owner} claims=${sharedState.claims}`;
}
