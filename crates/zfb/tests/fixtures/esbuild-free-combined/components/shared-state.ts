// One module instance backs both the eager island and the lazily imported
// panel (shape reused from the lazy-shared-island fixture).
export const sharedState = { owner: "unclaimed", claims: 0 };

export function claimSharedState(owner: string): void {
  sharedState.owner = owner;
  sharedState.claims += 1;
}
