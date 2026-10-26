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
import { DEPLOY_SLUGS } from "./config/navigation-groups.mjs";
import type { SidebarsConfig } from "@takazudo/zudo-doc/sidebar-utils";
import {
  canonicalNavSection,
  editorialForestFor,
  resolveNavigationScope,
  sidebarDefinitionFor,
} from "./config/navigation-scope.mjs";

export function createNavigationChrome(baseBindings: ChromeHostBindings) {
  const routeCtx = createRouteContext(routeContext as unknown as RouteContextPayload);
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
    const key = `${locale}:${scope?.id ?? "native"}`;
    const existing = pairs.get(key);
    if (existing) return existing;
    const sidebarsConfig: SidebarsConfig = {
      ...baseBindings.sidebarsConfig,
      "deploy-embed": ["deploy-embed", ...DEPLOY_SLUGS],
      // A scope's dedicated forest has one real landing-page root; the native
      // builder keeps all of it as navigation roots and starts at the scope's
      // configured view, so group rows link to their pages and Broaden reaches
      // real editorial ancestors.
      ...(scope ? { [scope.navSection]: sidebarDefinitionFor(scope) } : {}),
    };
    // Only this pair's Header/Sidebar (and version availability, unused while
    // zfb.config.ts sets no versions) read the overridden tree; breadcrumbs,
    // pagers and CategoryNav keep the URL-based route context.
    const context: ChromeContext = {
      ...routeCtx,
      components: {},
      hostBindings: { ...baseBindings, sidebarsConfig },
      ...(scope
        ? {
            buildNavTree: (...args: Parameters<typeof routeCtx.buildNavTree>) =>
              editorialForestFor(scope, routeCtx.buildNavTree(...args)),
          }
        : {}),
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
    return { ...props, navSection: canonicalNavSection(props.currentSlug, props.navSection) };
  }
  return {
    Header: (props: HeaderWithDefaultsProps) =>
      pair(editorialProps(props)).Header(editorialProps(props)),
    Sidebar: (props: SidebarWithDefaultsProps) =>
      pair(editorialProps(props)).Sidebar(editorialProps(props)),
  };
}
