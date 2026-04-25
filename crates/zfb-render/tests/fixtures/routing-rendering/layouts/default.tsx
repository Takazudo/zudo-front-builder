// Default layout. Wraps page output in a simple shell.
// Receives `meta` (from the page's `export const meta`) and
// `children` (the rendered page).

import Header from "../components/header";

export type DefaultLayoutProps = {
  meta?: { title?: string };
  children?: unknown;
};

export default function DefaultLayout({ meta, children }: DefaultLayoutProps) {
  return (
    <div className="layout layout-default">
      <Header title={meta?.title ?? "zfb"} />
      <main>{children}</main>
    </div>
  );
}
