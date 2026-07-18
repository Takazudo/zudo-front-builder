// Plain workspace-sibling module (issue #1711, Scenario D) — reached by an
// ordinary import, neither a `?raw` target nor a module-worker dependency.
// Proves the `client_script_siblings` registry (issue #1710) closes the DEV
// restart-free invalidation loop for this third sibling shape too, mirroring
// Scenarios A/B for `?raw` and worker deps above.
export const plainMarker = "ZFB_SIBLING_PLAIN_PAYLOAD_V1";
