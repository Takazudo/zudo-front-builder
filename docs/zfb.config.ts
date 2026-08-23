import { defineConfig } from "zfb/config";
import { zudoDoc } from "@takazudo/zudo-doc/config";
import { buildDocsSchema } from "./src/config/docs-schema";

export default defineConfig(
  zudoDoc({
    // ── Identity / URLs ────────────────────────────────────────────────
    siteName: "zfb",
    siteDescription:
      "The Rust engine under your content-site framework — router, renderer, content pipeline. Author in TypeScript/JSX, runs as a single binary.",
    siteUrl: "https://zfb.takazudomodular.com",
    githubUrl: "https://github.com/Takazudo/zudo-front-builder",
    // zudoDoc() shallow-merges: a supplied nested object REPLACES the package
    // default wholesale, so all five keys are required even though only
    // ogImage + twitterCard differ from it.
    metaTags: {
      description: true,
      keywords: false,
      ogImage: "/img/ogp.png",
      ogSiteName: true,
      twitterCard: "summary_large_image",
    },
    // v4 introduced `logo` (default "auto" = a generated mark seeded by
    // siteName). v3 had no such field and rendered public/img/logo.svg as the
    // home hero mask by convention; naming the asset explicitly reproduces
    // today's rendering. Neither v4 default is neutral.
    logo: "/img/logo.svg",

    // ── Content / i18n ─────────────────────────────────────────────────
    locales: {
      ja: { label: "JA", dir: "src/content/docs-ja" },
    },
    cjkFriendly: true,
    defaultLocaleOnlyPrefixes: [
      "/docs/changelog/",
      "/docs/claude/",
      "/docs/claude-md/",
      "/docs/claude-skills/",
      "/docs/claude-agents/",
      "/docs/claude-commands/",
    ],

    // ── Generation ─────────────────────────────────────────────────────
    sitemap: true,
    llmsTxt: true,
    claudeResources: { claudeDir: ".claude" },

    // ── Doc-page features ──────────────────────────────────────────────
    docMetainfo: true,
    docHistory: true,
    sidebarResizer: true,
    sidebarToggle: true,
    tocToggle: true,
    imageEnlarge: true,
    dynamicPageTransition: true,
    // Host-authored MDX components enter the package-owned chrome through
    // this bindings module. The route stubs also import it statically so zfb's
    // island scanner can discover its client components.
    chromeBindingsModule: "./src/chrome-bindings.tsx",

    // ── Chrome ─────────────────────────────────────────────────────────
    footer: {
      links: [
        {
          title: "Docs",
          items: [{ label: "Getting Started", href: "/docs/getting-started" }],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} <a href="https://x.com/Takazudo">Takazudo</a>. Built with <a href="https://takazudomodular.com/pj/zudo-doc">zudo-doc</a>.`,
    },
    headerNav: [
      { label: "Getting Started", path: "/docs/getting-started", categoryMatch: "getting-started" },
      { label: "Install", path: "/docs/install", categoryMatch: "install" },
      { label: "Concepts", path: "/docs/concepts", categoryMatch: "concepts" },
      { label: "Guides", path: "/docs/guides", categoryMatch: "guides" },
      { label: "Recipes", path: "/docs/recipes", categoryMatch: "recipes" },
      { label: "Reference", path: "/docs/api", categoryMatch: "api" },
      { label: "Architecture", path: "/docs/architecture", categoryMatch: "architecture" },
      { label: "Claude", path: "/docs/claude", categoryMatch: "claude" },
      {
        label: "Markdown Features",
        path: "/docs/markdown-features",
        categoryMatch: "markdown-features",
      },
      {
        label: "Changelog",
        path: "/docs/changelog",
        categoryMatch: "changelog",
        children: [
          { label: "zfb", path: "/docs/changelog/zfb" },
          { label: "zfb-runtime", path: "/docs/changelog/zfb-runtime" },
          {
            label: "zfb-adapter-cloudflare",
            path: "/docs/changelog/zfb-adapter-cloudflare",
          },
          { label: "create-zfb", path: "/docs/changelog/create-zfb" },
          { label: "zfb-md-wasm", path: "/docs/changelog/zfb-md-wasm" },
        ],
      },
    ],
    headerRightItems: [
      { type: "component", component: "github-link" },
      { type: "component", component: "theme-toggle" },
      { type: "component", component: "search" },
      { type: "component", component: "language-switcher" },
    ],

    // ── Escape hatch — the only one that still earns its keep ───────────
    // Keeps the `tier: "Core" | "Opt-in"` frontmatter enum validated. The
    // package default schema is otherwise a superset of ours and passthrough()s
    // unknown keys.
    buildDocsSchema,

    // ── Shell fields ───────────────────────────────────────────────────
    adapter: "@takazudo/zfb-adapter-cloudflare",
  }),
);
