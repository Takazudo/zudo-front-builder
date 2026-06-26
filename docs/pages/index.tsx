/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Page module for the site index route.
//
// Default-locale (EN) site index. Static route — no paths() export needed.
// Collects the EN docs tree and renders the site-map grid plus optional
// tag count.
//
// Data flow:
//   getCollection("docs")   [sync, zfb ADR-004]
//   → buildNavTree()        builds the nav tree for the sitemap grid
//   → collectTags()         counts unique tags for the tag section header
//   → DocLayoutWithDefaults renders the page with no sidebar/TOC

import { settings } from "@/config/settings";
import { defaultLocale } from "@/config/i18n";
import { withBase } from "@/utils/base";
import { buildNavTree, groupSatelliteNodes } from "@/utils/docs";
import { resolveNavSource } from "./lib/_nav-source-docs";
import { getCategoryOrder } from "@/utils/nav-scope";
import { collectTags } from "@/utils/tags";
import { toRouteSlug } from "@/utils/slug";
import { DocLayoutWithDefaults } from "@takazudo/zudo-doc/doclayout";
import type { JSX } from "preact";
import { FooterWithDefaults } from "./lib/_footer-with-defaults";
import { HeaderWithDefaults } from "./lib/_header-with-defaults";
import { HeadWithDefaults } from "./lib/_head-with-defaults";
import { composeMetaTitle } from "./lib/_compose-meta-title";
import { BodyEndIslands } from "./lib/_body-end-islands";
import { HomePageBody } from "./lib/_home-page-body";

export const frontmatter = { title: "Home" };

export default function IndexPage(): JSX.Element {
  const locale = defaultLocale;

  // Identity-stable nav source (draft-filtered, unlisted retained). navDocs is
  // pre-filtered (isNavVisible) and shared with the nav-tree fast-path.
  const { navDocs, categoryMeta } = resolveNavSource(locale, undefined);
  const tree = buildNavTree(navDocs, locale, categoryMeta);
  const categoryOrder = getCategoryOrder();
  const groupedTree = groupSatelliteNodes(tree, categoryOrder);

  const tagCount = collectTags(navDocs, (id, data) => data.slug ?? toRouteSlug(id)).size;

  const ctaNav = settings.headerNav[0] ?? null;
  const overviewHref = ctaNav ? withBase(ctaNav.path) : null;

  return (
    <DocLayoutWithDefaults
      title={composeMetaTitle(settings.siteName)}
      head={<HeadWithDefaults title={settings.siteName} />}
      lang={locale}
      noindex={settings.noindex}
      hideSidebar={true}
      hideToc={true}
      // Empty fragment suppresses DocLayoutWithDefaults' empty-data default
      // Sidebar island — its marker never hydrates for published-package
      // consumers (zfb#999) and zfb >= next.38 warns about it; the sidebar is
      // hidden on this page anyway (zudolab/zudo-doc#2057).
      sidebarOverride={<></>}
      headerOverride={<HeaderWithDefaults lang={locale} currentPath={withBase("/")} />}
      footerOverride={<FooterWithDefaults lang={locale} />}
      bodyEndComponents={<BodyEndIslands basePath={settings.base ?? "/"} />}
      enableClientRouter={settings.dynamicPageTransition}
    >
      <HomePageBody
        locale={locale}
        tree={groupedTree}
        categoryOrder={categoryOrder}
        tagCount={tagCount}
        overviewHref={overviewHref}
        tagsHref={withBase("/docs/tags")}
      />
    </DocLayoutWithDefaults>
  );
}
