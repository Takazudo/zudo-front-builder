/** @satisfies {import("@takazudo/zudo-sg/config").ZudoSgComposeOptions} */
export default {
  componentsRoots: [{ dir: "ui", importBase: "../../ui" }],
  registryOut: "./src/styleguide/sg-registry.ts",
  categoryOrder: ["Actions"],
  barrelIndex: null,
  previewStyles: "./src/styles/preview-entry.css",
  tokens: {
    // [tokensCssPath, colorsCssPath]; one file declares both groups here.
    cssFiles: ["./src/styles/ui-tokens.css", "./src/styles/ui-tokens.css"],
    manifestOut: "./src/styleguide/token-manifest.ts",
  },
};
