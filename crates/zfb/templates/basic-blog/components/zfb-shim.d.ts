/**
 * The zfb bundlers resolve bare `zfb` imports to `@takazudo/zfb`, while
 * TypeScript needs declarations for that separate module name. Keep these
 * entry points in sync with the package's public exports.
 */
declare module "zfb" {
  export * from "@takazudo/zfb";
}

declare module "zfb/runtime" {
  export * from "@takazudo/zfb/runtime";
}

declare module "zfb/content" {
  export * from "@takazudo/zfb/content";
}

declare module "zfb/paginate" {
  export * from "@takazudo/zfb/paginate";
}

declare module "zfb/config" {
  export * from "@takazudo/zfb/config";
}

declare module "zfb/plugins" {
  export * from "@takazudo/zfb/plugins";
}

declare module "zfb/frontmatter" {
  export * from "@takazudo/zfb/frontmatter";
}

declare module "zfb/slugify" {
  export * from "@takazudo/zfb/slugify";
}

declare module "zfb/zudo-react" {
  export * from "@takazudo/zfb/zudo-react";
}

declare module "zfb/zudo-react/jsx-runtime" {
  export * from "@takazudo/zfb/zudo-react/jsx-runtime";
}

declare module "zfb/zudo-react/jsx-dev-runtime" {
  export * from "@takazudo/zfb/zudo-react/jsx-dev-runtime";
}

declare module "zfb/zudo-react/server" {
  export * from "@takazudo/zfb/zudo-react/server";
}

declare module "zfb/zudo-react/client" {
  export * from "@takazudo/zfb/zudo-react/client";
}

declare module "zfb/zudo-react/testing" {
  export * from "@takazudo/zfb/zudo-react/testing";
}
