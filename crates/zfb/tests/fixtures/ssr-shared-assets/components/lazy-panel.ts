import { sharedState } from "./shared-state";

export function describeSharedState(): string {
  return `lazy owner=${sharedState.owner} claims=${sharedState.claims}`;
}
