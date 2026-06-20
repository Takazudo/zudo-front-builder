/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Shared home-page body content, used by both the default-locale root
// (pages/index.tsx) and the locale-prefixed index (pages/[locale]/index.tsx).
//
// Extracted to eliminate ~95% code duplication between the two page modules.
// Both modules provide locale-specific data (nav tree, tag count, hrefs) and
// pass them as props — the body renders them identically.

import { settings } from "@/config/settings";
import { t } from "@/config/i18n";
import { withBase } from "@/utils/base";
import type { NavNode } from "@/utils/docs";
import type { JSX, VNode } from "preact";
import { Island } from "@takazudo/zfb";
import SiteTreeNav from "@/components/site-tree-nav";

export interface HomePageBodyProps {
  locale: string;
  /** Grouped nav tree produced by groupSatelliteNodes(). */
  tree: NavNode[];
  /** Category ordering array from getCategoryOrder(). */
  categoryOrder: string[];
  tagCount: number;
  /** Base-prefixed href for the overview/first-header-nav link, or null. */
  overviewHref: string | null;
  /** Base-prefixed href for the tags index page (e.g. /docs/tags or /ja/docs/tags). */
  tagsHref: string;
}

/**
 * Renders the home-page hero + sitemap grid + optional tag section.
 * Shared between pages/index.tsx (EN) and pages/[locale]/index.tsx.
 */
export function HomePageBody({
  locale,
  tree,
  categoryOrder,
  tagCount,
  overviewHref,
  tagsHref,
}: HomePageBodyProps): JSX.Element {
  const logoUrl = withBase("/img/logo.svg");

  return (
    <>
      {/* Hero: logo left, title+desc+links right, block centered */}
      <div class="flex justify-center mb-vsp-xl">
        <div class="flex flex-col items-center text-center gap-hsp-md lg:flex-row lg:text-left lg:gap-hsp-xl">
          {/* Theme-adaptive logo: SVG used as a CSS mask over `bg-fg` so the
              foreground color follows the active theme (white on dark, black on
              light). The neighboring <h1>{settings.siteName}</h1> provides the
              accessible name; mirrors zudolab/zudo-design-token-lint#65. */}
          <div
            class="w-[320px] max-w-full aspect-[1200/630] bg-fg shrink-0"
            style={{
              WebkitMask: `url(${logoUrl}) center/contain no-repeat`,
              mask: `url(${logoUrl}) center/contain no-repeat`,
            }}
            aria-hidden="true"
          />
          <div>
            <h1 class="text-heading font-bold mb-vsp-2xs">{settings.siteName}</h1>
            <p class="text-muted text-small mb-vsp-sm">{settings.siteDescription}</p>
            <div class="flex items-center justify-center lg:justify-start gap-hsp-md text-small">
              {overviewHref && (
                <>
                  <a href={overviewHref} class="text-fg underline hover:text-accent">
                    {t("nav.overview", locale)}
                  </a>
                  <span class="text-muted">/</span>
                </>
              )}
              {settings.githubUrl && (
                <>
                  <a
                    href={settings.githubUrl as string}
                    class="inline-flex items-center gap-[0.3em] text-fg underline hover:text-accent"
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    <svg viewBox="0 0 16 16" aria-hidden="true" class="w-[1em] h-[1em] shrink-0">
                      <path
                        fill="currentColor"
                        d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"
                      />
                    </svg>
                    GitHub
                  </a>
                  <span class="text-muted">/</span>
                </>
              )}
              {/* @Takazudo link — established in #1453 (project-specific brand link).
                  The deploy was missing this trailing item, leaving a dangling "/" separator. */}
              <a
                href="https://x.com/Takazudo"
                class="text-fg underline hover:text-accent"
                target="_blank"
                rel="noopener noreferrer"
              >
                @Takazudo
              </a>
            </div>
          </div>
        </div>
      </div>

      {/* Sitemap grid — restored to the original SiteTreeNav island (refs #1453).
          The Astro reference used <Island when="idle"><SiteTreeNav ...></Island>.
          DocsSitemap (vertical <details> list) was incorrect; SiteTreeNav gives
          the responsive multi-column grid the reference renders. */}
      {
        Island({
          when: "idle",
          children: (
            <SiteTreeNav
              tree={tree}
              categoryOrder={categoryOrder}
              categoryIgnore={["inbox", "develop"]}
            />
          ),
        }) as unknown as VNode
      }

      {settings.docTags && tagCount > 0 && (
        <section class="mt-vsp-xl">
          <h2 class="text-title font-bold mb-vsp-md">{t("doc.allTags", locale)}</h2>
          {/* Distinct link label — heading says "All Tags"; link text uses
              "docs.browseAll" so the two are not identical (a11y: WCAG 2.4.6). */}
          <a href={tagsHref} class="text-accent underline hover:text-accent-hover">
            {t("docs.browseAll", locale)}
          </a>
        </section>
      )}
    </>
  );
}
