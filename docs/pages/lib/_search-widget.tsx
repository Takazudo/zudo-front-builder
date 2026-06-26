/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Host thin-stub — see @takazudo/zudo-doc/search-widget (docs-1.0-migration, S3).
// Injects the host's base path and re-exports the package SearchWidget.
// New props introduced in the package (searchUnavailableText, loadingIndexText,
// noResultsText) are made optional with English defaults so existing call sites
// (_header-with-defaults.tsx) continue to work without changes until S4 wires
// the locale-aware strings.
import type { JSX } from "preact";
import {
  SearchWidget as PackageSearchWidget,
  type SearchWidgetProps as PackageSearchWidgetProps,
} from "@takazudo/zudo-doc/search-widget";
import { withBase } from "@/utils/base";

export type SearchWidgetProps = Omit<
  PackageSearchWidgetProps,
  "base" | "searchUnavailableText" | "loadingIndexText" | "noResultsText"
> & {
  searchUnavailableText?: string;
  loadingIndexText?: string;
  noResultsText?: string;
};

/** Locale-aware search widget — thin host stub that injects the base path. */
export function SearchWidget(props: SearchWidgetProps): JSX.Element {
  return (
    <PackageSearchWidget
      {...props}
      base={withBase("/")}
      searchUnavailableText={props.searchUnavailableText ?? "Search unavailable"}
      loadingIndexText={props.loadingIndexText ?? "Loading search index…"}
      noResultsText={props.noResultsText ?? "No results found."}
    />
  );
}
