// Blog-specific layout. Same shape as default but adds a
// "blog" wrapper class so snapshots can prove `meta.layout`
// selection works.

import Header from "../components/header";

export type BlogLayoutProps = {
  meta?: { title?: string };
  children?: unknown;
};

export default function BlogLayout({ meta, children }: BlogLayoutProps) {
  return (
    <div className="layout layout-blog">
      <Header title={meta?.title ?? "Blog"} subtitle="Posts" />
      <article>{children}</article>
    </div>
  );
}
