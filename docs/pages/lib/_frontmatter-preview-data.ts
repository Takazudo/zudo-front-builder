// Host thin-stub — frontmatter-preview-data moved to the package (docs-1.0-migration, S3).
//
// Binds the host's `settings.frontmatterPreview` and
// `DEFAULT_FRONTMATTER_IGNORE_KEYS` to the package factory, then re-exports
// the resulting helper so all existing call sites remain unchanged.

import { createBuildFrontmatterPreviewEntries } from "@takazudo/zudo-doc/frontmatter-preview-data";
import { settings } from "@/config/settings";
import { DEFAULT_FRONTMATTER_IGNORE_KEYS } from "@/config/frontmatter-preview-defaults";

export const buildFrontmatterPreviewEntries = createBuildFrontmatterPreviewEntries({
  frontmatterPreview: settings.frontmatterPreview,
  defaultIgnoreKeys: DEFAULT_FRONTMATTER_IGNORE_KEYS,
});
