/**
 * Compose the canonical "<title> | <siteName>" page-title shape used by
 * both <title> (emitted by DocLayout) and og:title (emitted by
 * HeadWithDefaults).
 *
 * Thin host stub around @takazudo/zudo-doc/compose-meta-title (docs-1.0-migration, S3).
 * Binds `settings.siteName` once so all call sites remain unchanged.
 */
import { createComposeMetaTitle } from "@takazudo/zudo-doc/compose-meta-title";
import { settings } from "@/config/settings";

export const composeMetaTitle = createComposeMetaTitle(settings.siteName);
