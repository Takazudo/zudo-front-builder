import { defineConfig } from "zfb/config";
import { zudoDoc } from "@takazudo/zudo-doc/config";
import { buildDocsSchema } from "./src/config/docs-schema";

export default defineConfig(
  zudoDoc({
    // ── Identity / URLs ────────────────────────────────────────────────
    siteName: "zfb",
    siteDescription:
      "A Request/Response content engine: write TSX and MDX once for static HTML, Cloudflare Workers, or local apps.",
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
    home: {
      wide: false,
      introMarkdown: `Write your pages in TSX and MDX. zfb turns them into one Request/Response application: prerender it to static HTML, serve it on Cloudflare Workers, or run it as a local content server inside your own app.

For developers building content sites or adding content to a desktop app, zfb provides routing, rendering, and content collections as small, composable parts. The engine ships as a single Rust binary.

Read the [introduction](/docs/getting-started/introduction/) or explore the [three ways to run zfb](/docs/concepts/three-ways-to-run-zfb/).`,
      sitemapHeading: "",
    },

    // ── Content / i18n ─────────────────────────────────────────────────
    locales: {
      ja: {
        label: "JA",
        dir: "src/content/docs-ja",
        description:
          "Request/Response モデルのコンテンツエンジン。TSX と MDX を一度書けば、静的 HTML、Workers、ローカルアプリで使えます。",
        introMarkdown: `TSX と MDX で書いたページを、zfb が 1 つの Request/Response アプリケーションにまとめます。静的 HTML にプリレンダリングする、Cloudflare Workers で配信する、自分のアプリの中でローカルコンテンツサーバーとして動かす、という 3 つの使い方ができます。

コンテンツサイトを作る人や、デスクトップアプリにコンテンツを組み込みたい人のために、ルーティング、レンダリング、コンテンツコレクションを、小さく組み合わせやすい部品として提供します。エンジンは単一の Rust バイナリとして配布されます。

[はじめに](/ja/docs/getting-started/introduction/)と、[zfb を動かす 3 つの方法](/ja/docs/concepts/three-ways-to-run-zfb/)をご覧ください。`,
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
    headerNav: [
      { label: "Getting Started", path: "/docs/getting-started", categoryMatch: "getting-started" },
      { label: "Install", path: "/docs/install", categoryMatch: "install" },
      { label: "Concepts", path: "/docs/concepts", categoryMatch: "concepts" },
      { label: "Guides", path: "/docs/guides", categoryMatch: "guides" },
      { label: "Recipes", path: "/docs/recipes", categoryMatch: "recipes" },
      { label: "Reference", path: "/docs/api", categoryMatch: "api" },
      {
        label: "Playground",
        path: "/docs/playground",
        categoryMatch: "playground",
        children: [
          { label: "Index", path: "/docs/playground" },
          { label: "renderHtml", path: "/docs/playground/render" },
          { label: "compile", path: "/docs/playground/compile" },
          { label: "parseToAst", path: "/docs/playground/parse" },
          { label: "highlightCode", path: "/docs/playground/highlight" },
        ],
      },
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
