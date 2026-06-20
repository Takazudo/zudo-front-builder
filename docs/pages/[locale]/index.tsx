/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Page module for the locale-prefixed site index route.
//
// Non-default-locale site index. paths() emits one route per locale defined
// in settings.locales (never the default locale — that is handled by
// pages/index.tsx since prefixDefaultLocale is false).
//
// paths() contract (zfb ADR-004 — synchronous):
//   params: { locale: string }   — e.g. "ja"
//   props:  { locale }           — resolved locale passed to component
//
// Data flow (inside component — sync per ADR-004):
//   getCollection(`docs-${locale}`)  + base fallback merge
//   → buildNavTree()   → groupSatelliteNodes()
//   → collectTags()    → tag section

import { settings } from "@/config/settings";
import { withBase } from "@/utils/base";
import { buildNavTree, groupSatelliteNodes, loadCategoryMeta } from "@/utils/docs";
import { getCategoryOrder } from "@/utils/nav-scope";
import { collectTags } from "@/utils/tags";
import { toRouteSlug } from "@/utils/slug";
import { DocLayoutWithDefaults } from "@takazudo/zudo-doc/doclayout";
import type { JSX } from "preact";
import { resolveNavSource } from "../lib/_nav-source-docs";
import { FooterWithDefaults } from "../lib/_footer-with-defaults";
import { HeaderWithDefaults } from "../lib/_header-with-defaults";
import { HeadWithDefaults } from "../lib/_head-with-defaults";
import { composeMetaTitle } from "../lib/_compose-meta-title";
import { BodyEndIslands } from "../lib/_body-end-islands";
import { HomePageBody } from "../lib/_home-page-body";

export const frontmatter = { title: "Home" };

// ---------------------------------------------------------------------------
// paths() — synchronous (ADR-004)
// ---------------------------------------------------------------------------

/** Emit one route per non-default locale. */
export function paths(): Array<{
  params: { locale: string };
  props: { locale: string };
}> {
  return Object.keys(settings.locales).map((locale) => ({
    params: { locale },
    props: { locale },
  }));
}

// ---------------------------------------------------------------------------
// Page component
// ---------------------------------------------------------------------------

interface PageArgs {
  params: { locale: string };
  props: { locale: string };
}

export default function LocaleIndexPage({ params }: PageArgs): JSX.Element {
  const locale = params.locale;

  // Identity-stable, locale-first merge with EN fallback (shared `navDocs`
  // instance). categoryMeta is intentionally locale-dir-only here — this page
  // historically did NOT merge in base meta (unlike the locale doc route), so
  // we keep that exact behavior to preserve output.
  const { navDocs } = resolveNavSource(locale, undefined, {
    applyDefaultLocaleOnlyFilter: true,
    keepUnlisted: true,
  });
  const localeConfig = settings.locales[locale];
  const categoryMeta = localeConfig
    ? loadCategoryMeta(localeConfig.dir)
    : loadCategoryMeta(settings.docsDir);

  const tree = buildNavTree(navDocs, locale, categoryMeta);
  const categoryOrder = getCategoryOrder();
  const groupedTree = groupSatelliteNodes(tree, categoryOrder);

  const tagCount = collectTags(navDocs, (id, data) => data.slug ?? toRouteSlug(id)).size;

  const ctaNav = settings.headerNav[0] ?? null;
  const overviewHref = ctaNav ? withBase(`/${locale}${ctaNav.path}`) : null;

  return (
    <DocLayoutWithDefaults
      title={composeMetaTitle(settings.siteName)}
      head={<HeadWithDefaults title={settings.siteName} />}
      lang={locale}
      noindex={settings.noindex}
      hideSidebar={true}
      hideToc={true}
      headerOverride={<HeaderWithDefaults lang={locale} currentPath={withBase(`/${locale}/`)} />}
      footerOverride={<FooterWithDefaults lang={locale} />}
      bodyEndComponents={<BodyEndIslands basePath={settings.base ?? "/"} />}
    >
      <HomePageBody
        locale={locale}
        tree={groupedTree}
        categoryOrder={categoryOrder}
        tagCount={tagCount}
        overviewHref={overviewHref}
        tagsHref={withBase(`/${locale}/docs/tags`)}
      />
    </DocLayoutWithDefaults>
  );
}
