import { signal } from "@takazudo/zfb/zudo-react";
import { sharedState } from "./shared-state";

export function describeSharedState(): string {
  const seen = signal(`lazy saw owner=${sharedState.owner} claims=${sharedState.claims}`);
  return seen.value;
}
