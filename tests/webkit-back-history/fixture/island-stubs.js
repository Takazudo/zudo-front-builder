// Stub module for @takazudo/zfb/runtime.
// In a full zfb site, these functions manage island hydration lifecycle.
// In this standalone fixture there are no islands, so all three are no-ops.
// The ClientRouter calls them unconditionally on every navigation, so they
// must exist — the stubs satisfy the call without doing anything.
export function cancelPendingIslands() {}
export function mountNewIslands() {}
export function unmountIslands() {}
