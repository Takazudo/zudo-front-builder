import { basename, dirname } from "node:path";

const generatedRootNames = new Set(["dist", ".zfb", ".zfb-build"]);
const cssOutputNames = new Set(["generated.css", "node-free.css"]);

export function fixtureCopyFilter(source, { excludeCssOutputs = false } = {}) {
  return (path) => {
    // A dependency's dist/ can contain scanned package-root sources.
    if (dirname(path) !== source) return true;
    const name = basename(path);
    return (
      !generatedRootNames.has(name) &&
      !name.startsWith(".zfb-dev-") &&
      !(excludeCssOutputs && cssOutputNames.has(name))
    );
  };
}
