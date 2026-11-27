import { astroTextPredicate } from "./astro-text.mjs";
export const ASTRO_FOOTER = "Same content. Same design. Three build systems.";
export const ASTRO_FOOTER_SELECTOR = ".site-footer > p";
export function astroFooterPredicate(value) {
  return astroTextPredicate(value, ASTRO_FOOTER_SELECTOR);
}
export {
  matchesAstroText as matchesAstroFooter,
  astroProducerMatches as footerProducerMatches,
} from "./astro-text.mjs";
