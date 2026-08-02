import { Note, Tip, Important, Warning, Caution } from "~/components/callout";

/**
 * Project-wide component overrides for every rendered content entry.
 *
 * zfb discovers this file at the project root and installs its default
 * export before pages render, so `<entry.Content />` picks these up with no
 * per-call wiring. Merge order is `defaultComponents` → this map →
 * the `components` prop, so a page can still override any of these locally.
 *
 * The five alert names are required: `markdown.features.githubAlerts`
 * rewrites `> [!NOTE]` blockquotes into these components, and an unresolved
 * PascalCase name throws while rendering the post.
 */
export default {
  Note,
  Tip,
  Important,
  Warning,
  Caution,
};
