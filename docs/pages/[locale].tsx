/** @jsxRuntime automatic */
/** @jsxImportSource preact */
import { routeContext } from "virtual:zudo-doc-route-context";
import { createRouteContext, type RouteContextPayload } from "@takazudo/zudo-doc/route-context";
import { createChrome } from "@takazudo/zudo-doc/chrome";
import { prepareHomeData } from "@takazudo/zudo-doc/home-page";
import { chromeBindings } from "virtual:zudo-doc-chrome-bindings";
import "../src/chrome-bindings";

const routeCtx = createRouteContext(routeContext as unknown as RouteContextPayload);
const { HomePageView } = createChrome(routeCtx, chromeBindings);
export const frontmatter = { title: "Home" };
export function paths() {
  return Object.keys(routeCtx.settings.locales).map((locale) => ({
    params: { locale },
    props: { locale },
  }));
}
export default function LocaleHome({ params }: { params: { locale: string } }) {
  const { locale } = params;
  const filtered = prepareHomeData(routeCtx, locale);
  // Keep native home rendering and URL policy. Include EN-only compact links,
  // while retaining the locale-filtered tag inventory used by native tag routes.
  const navigation = prepareHomeData(routeCtx, locale, {
    navSourceOptions: { applyDefaultLocaleOnlyFilter: false, keepUnlisted: true },
  });
  return (
    <HomePageView
      {...navigation}
      locale={locale}
      tags={filtered.tags}
      tagCount={filtered.tagCount}
      wide={routeCtx.settings.home?.wide ?? false}
    />
  );
}
