import { routeContext } from "virtual:zudo-doc-route-context";
import type { RouteContextPayload } from "@takazudo/zudo-doc/route-context";
import type { PlaygroundProps } from "./components/playground/host-copy";
import { createNavigationChrome } from "./navigation-chrome";
import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import DesignSystemPlayground from "./components/playground/design-system-playground";
import CompilePlayground from "./components/playground/compile-playground";
import DesignDiscovery from "./components/playground/design-discovery";
import HighlightPlayground from "./components/playground/highlight-playground";
import ParsePlayground from "./components/playground/parse-playground";
import RenderPlayground from "./components/playground/render-playground";
import WindPreviewEnhancer from "./components/wind-preview-enhancer";

const playgroundBasePath = (routeContext as unknown as RouteContextPayload).settings.base;

// Component-map entries keep the scanner's Fragment-wrapped fixed-target shape.
function DesignSystemPlaygroundIsland() {
  return (
    <>
      <Island when="visible">
        <DesignSystemPlayground />
      </Island>
    </>
  );
}

function RenderPlaygroundIsland({ locale = "en" }: PlaygroundProps) {
  return (
    <>
      <Island when="visible">
        <RenderPlayground locale={locale} basePath={playgroundBasePath} />
      </Island>
    </>
  );
}

function CompilePlaygroundIsland({ locale = "en" }: PlaygroundProps) {
  return (
    <>
      <Island when="visible">
        <CompilePlayground locale={locale} basePath={playgroundBasePath} />
      </Island>
    </>
  );
}

function ParsePlaygroundIsland({ locale = "en" }: PlaygroundProps) {
  return (
    <>
      <Island when="visible">
        <ParsePlayground locale={locale} basePath={playgroundBasePath} />
      </Island>
    </>
  );
}

function HighlightPlaygroundIsland({ locale = "en" }: PlaygroundProps) {
  return (
    <>
      <Island when="visible">
        <HighlightPlayground locale={locale} basePath={playgroundBasePath} />
      </Island>
    </>
  );
}

const linkClass = "text-fg underline hover:text-accent";

const HomeExtras = ({ locale }: { locale: string }) => {
  const label = locale === "ja" ? "zfb で作られたもの: " : "Built on zfb: ";
  const zudoDocTitle = locale === "ja" ? "ドキュメントフレームワーク" : "documentation framework";
  const ccResDocTitle = locale === "ja" ? "デスクトップアプリ" : "desktop app";
  return (
    <span>
      <span>{label}</span>
      <a href="https://github.com/zudolab/zudo-doc" title={zudoDocTitle} class={linkClass}>
        zudo-doc
      </a>
      {" · "}
      <a href="https://github.com/Takazudo/ccresdoc" title={ccResDocTitle} class={linkClass}>
        CCResDoc
      </a>
    </span>
  );
};

const baseBindings = defineChromeBindings({
  mdxExtras: {
    DesignSystemPlayground: DesignSystemPlaygroundIsland,
    RenderPlayground: RenderPlaygroundIsland,
    CompilePlayground: CompilePlaygroundIsland,
    ParsePlayground: ParsePlaygroundIsland,
    HighlightPlayground: HighlightPlaygroundIsland,
    WindPreviewEnhancer,
    DesignDiscovery,
  },
  homeExtras: HomeExtras,
});

export const chromeBindings = {
  ...baseBindings,
  ...defineChromeBindings(createNavigationChrome(baseBindings)),
};
