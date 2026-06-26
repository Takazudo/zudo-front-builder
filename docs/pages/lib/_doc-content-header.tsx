/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Content header for entry doc pages.
//
// NOT collapsed to the package factory (createDocContentHeader) because this
// host has a project-specific `tier` badge (Core / Opt-in) that appears between
// the <h1> and the metadata block. The package factory does not support injecting
// content between those two elements. Uses the already-collapsed factory stubs
// for sub-components (DocMetainfoArea, DocTagsArea, FrontmatterPreview).

import type { JSX } from "preact";
import { t } from "@/config/i18n";
import { FrontmatterPreview } from "@takazudo/zudo-doc/metainfo";
import { frontmatterRenderers } from "@/config/frontmatter-preview-renderers";
import { DocMetainfoArea } from "./_doc-metainfo-area";
import { DocTagsArea } from "./_doc-tags-area";
import { buildFrontmatterPreviewEntries } from "./_frontmatter-preview-data";
import type { DocPageEntry } from "./doc-page-props";

export interface DocContentHeaderProps {
  entry: DocPageEntry;
  slug: string;
  locale: string;
  isFallback?: boolean;
  version?: string;
}

/**
 * Content header block for entry doc pages: h1, tier badge (project-specific),
 * doc-metainfo, tag chips, optional locale-fallback notice, description, and
 * frontmatter preview table.
 */
export function DocContentHeader({
  entry,
  slug,
  locale,
  isFallback = false,
  version,
}: DocContentHeaderProps): JSX.Element {
  return (
    <>
      <h1 class="text-heading font-bold mb-vsp-xs">{entry.data.title}</h1>

      {/* Feature tier badge — "Core" / "Opt-in" — shown on markdown-features
          pages that set the `tier` frontmatter key (issue #877 step 6).
          Placed here so it appears between h1 and metadata on both EN and JA
          routes. The package factory does not render this badge; kept here. */}
      {entry.data.tier && (
        <span
          class={`inline-block mb-vsp-xs px-hsp-xs py-0.5 text-caption font-medium rounded border ${
            entry.data.tier === "Core"
              ? "border-accent/40 bg-accent/10 text-accent"
              : "border-muted bg-surface text-muted"
          }`}
        >
          {entry.data.tier}
        </span>
      )}

      {!version && <DocMetainfoArea slug={slug} locale={locale} isFallback={isFallback} />}
      {!version && <DocTagsArea slug={slug} locale={locale} tags={entry.data.tags} />}

      {isFallback && !entry.data.generated && (
        <div
          class="mb-vsp-md border border-info/30 bg-info/5 px-hsp-lg py-vsp-sm text-small text-muted rounded"
          role="note"
        >
          {t("doc.fallbackNotice", locale)}
        </div>
      )}

      {entry.data.description && (
        <p class="mb-vsp-lg text-title text-muted">{entry.data.description}</p>
      )}

      <FrontmatterPreview
        entries={buildFrontmatterPreviewEntries(entry.data)}
        title={t("frontmatter.preview.title", locale)}
        keyColLabel={t("frontmatter.preview.keyCol", locale)}
        valueColLabel={t("frontmatter.preview.valueCol", locale)}
        renderers={frontmatterRenderers}
        data={entry.data as Record<string, unknown>}
        locale={locale}
      />
    </>
  );
}
