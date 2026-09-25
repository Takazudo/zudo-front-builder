// The pnpm-private transitive dependency #3133 describes: reachable along
// >= 3 logical `node_modules/leftpad-priv` paths (ui, shared-utils,
// shared-icons) that all resolve to this one physical directory under
// `node_modules/.pnpm/leftpad-priv@1.0.0/node_modules/leftpad-priv` — the
// `visited` multiplier in `extend_node_modules_dependency_staging` is keyed
// on the *logical* root, so this file gets scanned once per logical path
// today.
export default "LEFTPAD_PRIV_MARKER";
