/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Host thin-stub — see @takazudo/zudo-doc/inline-version-switcher (dormant:
// versioning is not currently configured for these docs).
import { createInlineVersionSwitcher } from "@takazudo/zudo-doc/inline-version-switcher";
import { settings } from "@/config/settings";
import { defaultLocale, t } from "@/config/i18n";
import { docsUrl, versionedDocsUrl, withBase } from "@/utils/base";

export { type InlineVersionSwitcherVersionEntry } from "@takazudo/zudo-doc/inline-version-switcher";

export const buildInlineVersionSwitcher = createInlineVersionSwitcher({
  settings,
  defaultLocale,
  t,
  docsUrl,
  versionedDocsUrl,
  withBase,
});
