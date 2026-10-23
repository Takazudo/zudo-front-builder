// One module instance must back both the eager island and the lazily imported
// panel; a second copy would reset `owner` to "unclaimed" in the lazy view.
export const sharedState = { owner: "unclaimed", claims: 0 };

export function claimSharedState(owner: string): void {
  sharedState.owner = owner;
  sharedState.claims += 1;
}
