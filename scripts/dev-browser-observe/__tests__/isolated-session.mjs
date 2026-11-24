import { dirname, join } from "node:path";
import { execute as runSession } from "../lib/run.mjs";

// Each fixture owns its admission record, including any simulated failed stop.
export function execute(options) {
  return runSession({
    ...options,
    hooks: {
      admissionPath: join(dirname(options.profilePath), "active-session.json"),
      ...options.hooks,
    },
  });
}
