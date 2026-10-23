// Imports only first-party state shared with the eager island. Under Rolldown's
// common-chunk merging this exact shape folded `shared-state` into the islands
// entry, so this chunk imported "./islands.js" (#4444); importing the
// zudo-react runtime here as well kept a separate common chunk instead.
import { sharedState } from "./shared-state";

export function describeSharedState(): string {
  return `lazy saw owner=${sharedState.owner} claims=${sharedState.claims}`;
}
