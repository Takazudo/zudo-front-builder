import { routeContext } from "virtual:zudo-doc-route-context";
import { createRouteContext, type RouteContextPayload } from "@takazudo/zudo-doc/route-context";
import type { ChromeContext, ChromeHostBindings } from "@takazudo/zudo-doc/factory-context";
import {
  createHeaderWithDefaults,
  type HeaderWithDefaultsProps,
} from "@takazudo/zudo-doc/header-with-defaults";
import {
  createSidebarWithDefaults,
  type SidebarWithDefaultsProps,
} from "@takazudo/zudo-doc/sidebar-with-defaults";
import type { SidebarDefinition, SidebarsConfig } from "@takazudo/zudo-doc/sidebar-utils";
import { resolveCategoryOwner } from "@takazudo/zudo-doc/nav-scope";
import {
  canonicalNavSection,
  editorialForestFor,
  resolveNavigationScope,
  sectionForest,
  sidebarDefinitionFor,
} from "./config/navigation-scope.mjs";

export function createNavigationChrome(baseBindings: ChromeHostBindings) {
  const routeCtx = createRouteContext(routeContext as unknown as RouteContextPayload);
  const categoryOrder = routeCtx.getCategoryOrder().filter((match) => match !== "!");
  const pairs = new Map<
    string,
    {
      Header: ReturnType<typeof createHeaderWithDefaults>;
      Sidebar: ReturnType<typeof createSidebarWithDefaults>;
    }
  >();
  function pair(props: HeaderWithDefaultsProps | SidebarWithDefaultsProps) {
    const scope = resolveNavigationScope(props.currentSlug, props.navSection);
    const locale = props.lang ?? routeCtx.defaultLocale;
    const key = `${locale}:${scope?.id ?? `native:${props.navSection ?? ""}`}`;
    const existing = pairs.get(key);
    if (existing) return existing;
    const sidebarsConfig: SidebarsConfig = {
      // The host binding and the .mjs scope helper are loosely typed.
      ...(baseBindings.sidebarsConfig as SidebarsConfig | undefined),
      // A scope's dedicated forest has one real landing-page root; the native
      // builder keeps all of it as navigation roots and starts at the scope's
      // configured view, so group rows link to their pages and Broaden reaches
      // real editorial ancestors.
      ...(scope
        ? {
            [scope.navSection]: sidebarDefinitionFor(scope) as SidebarDefinition,
          }
        : {}),
    };
    // Only this pair's Header/Sidebar (and version availability, unused while
    // zfb.config.ts sets no versions) read the overridden tree; breadcrumbs,
    // pagers and CategoryNav keep the URL-based route context.
    const context: ChromeContext = {
      ...routeCtx,
      components: {},
      hostBindings: { ...baseBindings, sidebarsConfig },
      buildNavTree: (...args: Parameters<typeof routeCtx.buildNavTree>) => {
        const tree = routeCtx.buildNavTree(...args);
        if (scope) return editorialForestFor(scope, tree);
        const navSection = props.navSection;
        const candidates =
          navSection && !categoryOrder.includes(navSection)
            ? [...categoryOrder, navSection]
            : categoryOrder;
        return sectionForest(tree, navSection, (slug: string) =>
          resolveCategoryOwner(slug, candidates),
        );
      },
    };
    const created = {
      Header: createHeaderWithDefaults(context),
      Sidebar: createSidebarWithDefaults(context),
    };
    pairs.set(key, created);
    return created;
  }
  function editorialProps<T extends HeaderWithDefaultsProps | SidebarWithDefaultsProps>(
    props: T,
  ): T {
    return {
      ...props,
      navSection: canonicalNavSection(props.currentSlug, props.navSection),
    };
  }
  return {
    Header: (props: HeaderWithDefaultsProps) =>
      pair(editorialProps(props)).Header(editorialProps(props)),
    Sidebar: (props: SidebarWithDefaultsProps) =>
      pair(editorialProps(props)).Sidebar(editorialProps(props)),
  };
}
