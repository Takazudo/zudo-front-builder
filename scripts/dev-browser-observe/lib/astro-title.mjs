import { astroTextPredicate } from "./astro-text.mjs";

export const ASTRO_TITLE = "Finding useful signals in build output · 00005";
export const TITLE_DETAIL = "/articles/article-00005/";
export const TITLE_SELECTOR = ".article-header h1";
export const TITLE_RELATED = 'section.related .post-card h3 a[href="/articles/article-00005/"]';
const card = '.card-grid .post-card h3 a[href="/articles/article-00005/"]';
const detail = (slug) => "/articles/article-" + slug + "/";
export const TITLE_SUPPLEMENTAL = [
  ["/", card],
  ["/category/performance/", card],
  ["/archive/1/", card],
  [detail("00075"), TITLE_RELATED],
  [detail("00085"), TITLE_RELATED],
  [detail("00095"), TITLE_RELATED],
];

export function validateTitlePlan(plan) {
  const edited = ASTRO_TITLE + " " + plan.token;
  const matches = (predicate, selector) =>
    JSON.stringify(predicate) === JSON.stringify(astroTextPredicate(edited, selector));
  if (
    plan.targetPath !== TITLE_DETAIL ||
    !matches(plan.predicate, TITLE_SELECTOR) ||
    plan.supplemental.length !== TITLE_SUPPLEMENTAL.length ||
    !plan.supplemental.every(
      (q, i) =>
        q.path === TITLE_SUPPLEMENTAL[i][0] && matches(q.predicate, TITLE_SUPPLEMENTAL[i][1]),
    )
  )
    throw new Error("unexpected fixed Astro title route or predicate");
}
