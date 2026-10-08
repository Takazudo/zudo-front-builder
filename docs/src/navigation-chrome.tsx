/** @jsxRuntime automatic */
/** @jsxImportSource preact */
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
import { ARCHITECTURE_GROUPS } from "./config/navigation-groups.mjs";
import { resolveNavigationScope, selectNavigationTree } from "./config/navigation-scope.mjs";

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
    const context: ChromeContext = {
      ...routeCtx,
      components: {},
      hostBindings: {
        ...baseBindings,
        sidebarsConfig: {
          ...baseBindings.sidebarsConfig,
          architecture: ARCHITECTURE_GROUPS.map((group) => ({
            type: "category",
            label: locale === "ja" ? group.titleJa : group.title,
            items: [group.slug, ...group.members],
          })),
        },
      },
      ...(scope
        ? {
            buildNavTree: (...args: Parameters<typeof routeCtx.buildNavTree>) =>
              selectNavigationTree(routeCtx.buildNavTree(...args), scope),
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
  return {
    Header: (props: HeaderWithDefaultsProps) => pair(props).Header(props),
    Sidebar: (props: SidebarWithDefaultsProps) => pair(props).Sidebar(props),
  };
}
