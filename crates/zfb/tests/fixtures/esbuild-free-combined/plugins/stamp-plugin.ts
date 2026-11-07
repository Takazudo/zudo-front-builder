import fs from "node:fs";
import path from "node:path";

// #4636: a TypeScript plugin entry. Its virtual module reaches the SSR page,
// and its postBuild hook writes a receipt into the output directory.
interface StampContext {
  addVirtualModule(specifier: string, loader: () => string): void;
}

interface PostBuildContext {
  outDir: string;
  routes?: { routes: ReadonlyArray<{ output: string }> };
}

const STAMP = "ZFB_TS_PLUGIN_STAMP" as const;

export default {
  name: "combined-ts-stamp",
  setup({ addVirtualModule }: StampContext): void {
    addVirtualModule(
      "virtual:combined-stamp",
      () => `export const stamp = ${JSON.stringify(STAMP)};`,
    );
  },
  postBuild({ outDir, routes }: PostBuildContext): void {
    const paths = (routes?.routes ?? []).map((route) => route.output).sort();
    fs.writeFileSync(
      path.join(outDir, "ts-plugin-postbuild.json"),
      `${JSON.stringify({ stamp: STAMP, paths })}\n`,
    );
  },
};
