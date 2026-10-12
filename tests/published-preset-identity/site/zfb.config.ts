// Published-preset identity fixture (#3331): the published zudo-doc preset
// composed with the published zudo-sg engine, built by the workspace zfb binary.
import { defineConfig } from "@takazudo/zfb/config";
import { zudoDoc } from "@takazudo/zudo-doc/config";
import { withZudoSg } from "@takazudo/zudo-sg/config";
import zudoSgConfig from "./zudo-sg.config.mjs";

export default defineConfig(
  withZudoSg(
    zudoDoc({
      siteName: "Preset identity fixture",
      base: "/",
      favicon: "auto",
      mermaid: false,
      strictContentBridge: true,
      dynamicPageTransition: true,
      chromeBindingsModule: "./pages/lib/_chrome-bindings.tsx",
      headerNav: [{ label: "Guide", path: "/docs/guide/probe", categoryMatch: "guide" }],
    }),
    zudoSgConfig,
  ),
);
