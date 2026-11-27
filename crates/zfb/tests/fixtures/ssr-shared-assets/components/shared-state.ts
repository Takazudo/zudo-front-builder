export const sharedState = { owner: "unclaimed", claims: 0 };

export function claimSharedState(owner: string): void {
  sharedState.owner = owner;
  sharedState.claims += 1;
}
