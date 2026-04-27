export type {
  HeaderNavChildItem,
  HeaderNavItem,
  ColorModeConfig,
  HtmlPreviewConfig,
  LocaleConfig,
  VersionConfig,
  FooterConfig,
  FrontmatterPreviewConfig,
  TagPlacement,
} from "./settings-types";
import type {
  HeaderNavItem,
  ColorModeConfig,
  HtmlPreviewConfig,
  LocaleConfig,
  VersionConfig,
  FooterConfig,
  FrontmatterPreviewConfig,
  TagPlacement,
  TagGovernanceMode,
} from "./settings-types";

export const settings = {
  colorScheme: "Default Dark",
  colorMode: {
    defaultMode: "dark",
    lightScheme: "Default Light",
    darkScheme: "Default Dark",
    respectPrefersColorScheme: true,
  } satisfies ColorModeConfig,
  defaultLocale: "en",
  siteName: "Docs",
  siteDescription: "" as string,
  base: "/pj/zudo-front-builder",
  trailingSlash: false as boolean,
  noindex: false as boolean,
  editUrl: false as string | false,
  siteUrl: "" as string,
  docsDir: "src/content/docs",
  locales: {
    ja: { label: "JA", dir: "src/content/docs-ja" },
  } as Record<string, LocaleConfig>,
  mermaid: true,
  sitemap: false,
  docMetainfo: false,
  docTags: false,
  llmsTxt: true,
  math: false,
  cjkFriendly: true as boolean,
  onBrokenMarkdownLinks: "warn" as "warn" | "error" | "ignore",
  aiAssistant: false as boolean,
  docHistory: true,
  colorTweakPanel: false as boolean,
  sidebarResizer: true as boolean,
  sidebarToggle: true as boolean,
  htmlPreview: undefined as HtmlPreviewConfig | undefined,
  /**
   * Optional URL of the project's GitHub repository. When set, the header
   * renders a GitHub link icon. Leave undefined to omit the link.
   */
  githubUrl: undefined as string | undefined,
  /**
   * Frontmatter preview block, rendered above the doc body.
   * `false` disables the feature entirely. An object enables it and may
   * customise which keys are hidden via `ignoreKeys` / `extraIgnoreKeys`.
   */
  frontmatterPreview: false as false | FrontmatterPreviewConfig,
  /**
   * Where the per-page tag list is rendered. Undefined hides it.
   * - `"after-title"` — directly below the page title.
   * - `"before-pager"` — directly above the prev/next pager at the foot.
   */
  tagPlacement: undefined as TagPlacement | undefined,
  /** Master on/off switch for consulting `tag-vocabulary.ts` at runtime. */
  tagVocabulary: false as boolean,
  /** Tag governance enforcement level. See `TagGovernanceMode` for values. */
  tagGovernance: "off" as TagGovernanceMode,
  versions: [] as VersionConfig[],
  claudeResources: {
    claudeDir: ".claude",
  } as { claudeDir: string; projectRoot?: string } | false,
  footer: {
    links: [
      {
        title: "Docs",
        items: [{ label: "Getting Started", href: "/docs/getting-started" }],
      },
    ],
    copyright: "Copyright © 2026 Your Name. Built with zudo-doc.",
  } satisfies FooterConfig as FooterConfig | false,
  headerNav: [
    { label: "Getting Started", path: "/docs/getting-started", categoryMatch: "getting-started" },
    { label: "Changelog", path: "/docs/changelog", categoryMatch: "changelog" },
  ] as HeaderNavItem[],
};
