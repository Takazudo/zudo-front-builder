import { defineConfig } from "zfb/config";
import { zudoDoc } from "@takazudo/zudo-doc/config";
import { buildDocsSchema } from "./src/config/docs-schema";

export default defineConfig(
  zudoDoc({
    // ── Identity / URLs ────────────────────────────────────────────────
    siteName: "zfb",
    siteDescription:
      "Almost all everyday frontend development in one CLI: pages, content, interactivity, styles and production builds.",
    siteUrl: "https://zfb.takazudomodular.com",
    // Browser CI also builds this supported setting at a non-root prefix.
    base: "/",
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
    home: {
      wide: true,
      introMarkdown: `Almost all everyday frontend development in one CLI. Create a site, develop pages, check content and types, build, and preview with zfb. Write TSX, MDX, Markdown or HTML; add zudo-react interactivity and zudo-wind styles as needed.

Small, composable APIs remain the foundation. The standalone binary and npm package deliver the same CLI; full TypeScript checking and Cloudflare tooling still need Node.js.

Start with [your first site](/docs/getting-started/your-first-site/), [Build with zfb](/docs/build-with-zfb/), or [Deploy & Embed](/docs/deploy-embed/)`,
      sitemapHeading: "",
    },

    // ── Content / i18n ─────────────────────────────────────────────────
    locales: {
      ja: {
        label: "JA",
        dir: "src/content/docs-ja",
        description:
          "Request/Response モデルのコンテンツエンジン。TSX と MDX を一度書けば、静的 HTML、Workers、ローカルアプリで使えます。",
        introMarkdown: `日々のフロントエンド開発のほとんどを 1 つの CLI で。zfb でサイトを作成し、開発、コンテンツと型のチェック、ビルド、プレビューまで進めます。TSX、MDX、Markdown、HTML でページを書き、必要に応じて zudo-react と zudo-wind を組み合わせます。

基盤は小さく組み合わせやすい API です。スタンドアロンと npm は同じ CLI の配布方法です。完全な TypeScript チェックと Cloudflare のツールには Node.js が必要です。

[最初のサイト](/ja/docs/getting-started/your-first-site/)、[zfb で作る](/ja/docs/build-with-zfb/)、[デプロイと組み込み](/ja/docs/deploy-embed/)から始めてください。`,
      },
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
    siteTreeNavSecondary: [
      "architecture",
      "guides",
      "markdown-features",
      "playground",
      "changelog",
      "claude",
    ],
    // The 56-page "Utility reference" group otherwise dominates the home tree.
    siteTreeNavInitiallyCollapsed: ["zudo-wind/utilities"],
    headerNav: [
      { label: "Getting Started", path: "/docs/getting-started", categoryMatch: "getting-started" },
      { label: "Build with zfb", path: "/docs/build-with-zfb", categoryMatch: "build-with-zfb" },
      {
        label: "zudo-wind",
        path: "/docs/zudo-wind",
        categoryMatch: "zudo-wind",
        children: [
          { label: "Index", path: "/docs/zudo-wind" },
          { label: "Learn", path: "/docs/zudo-wind/learn" },
          { label: "Utility reference", path: "/docs/zudo-wind/utilities" },
          { label: "Tailwind compatibility", path: "/docs/zudo-wind/compatibility" },
        ],
      },
      { label: "zudo-react", path: "/docs/zudo-react", categoryMatch: "zudo-react" },
      { label: "Deploy & Embed", path: "/docs/deploy-embed", categoryMatch: "deploy-embed" },
      { label: "Reference", path: "/docs/api", categoryMatch: "api" },
      { label: "Recipes", path: "/docs/recipes", categoryMatch: "recipes" },
      { label: "Architecture", path: "/docs/architecture", categoryMatch: "architecture" },
      { label: "Migration & guides", path: "/docs/guides", categoryMatch: "guides" },
      {
        label: "Markdown Features",
        path: "/docs/markdown-features",
        categoryMatch: "markdown-features",
      },
      {
        label: "Playground",
        path: "/docs/playground",
        categoryMatch: "playground",
        children: [
          { label: "Index", path: "/docs/playground" },
          { label: "Design system", path: "/docs/playground/design-system" },
          { label: "renderHtml", path: "/docs/playground/render" },
          { label: "compile", path: "/docs/playground/compile" },
          { label: "parseToAst", path: "/docs/playground/parse" },
          { label: "highlightCode", path: "/docs/playground/highlight" },
        ],
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
      { label: "Claude", path: "/docs/claude", categoryMatch: "claude" },
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
    // Deep-merged over the package's Wind defaults (sm/lg/xl breakpoints).
    wind: {
      // The playground layouts keep their former 768px two-column step.
      breakpoints: { md: { minWidthPx: 768 } },
      sources: {
        exclude: [
          // Preview samples compile against their own embedded Wind configs.
          "src/content/docs/zudo-wind/**",
          "src/content/docs-ja/zudo-wind/**",
          // Generated starter code for the visitor's own design system.
          "src/components/playground/design-workshop/model.js",
        ],
      },
      // Design-workshop CSS owns these names, which overlap utility roots.
      authoredClasses: { "text-button": true, "start-view": true, "start-lower": true },
    },
  }),
);
