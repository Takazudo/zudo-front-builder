import type { ComponentChildren } from "preact";

import ThemeToggle from "../components/theme-toggle";
import "../styles/global.css";

type Props = {
  title?: string;
  children: ComponentChildren;
};

/**
 * The single shared chrome for every page in the example. The `"use client"`
 * boundary is at `components/theme-toggle.tsx`, so this layout itself stays
 * a plain server component — only the toggle ships JS to the browser.
 */
export default function DefaultLayout({
  title = "basic-blog · zfb example",
  children,
}: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{title}</title>
      </head>
      <body>
        <header class="site-header">
          <a class="site-title" href="/">
            basic-blog
          </a>
          <nav class="site-nav">
            <a href="/blog/page/1">Posts</a>
            <ThemeToggle />
          </nav>
        </header>
        <main class="site-main">{children}</main>
        <footer class="site-footer">
          <p>
            zfb basic-blog example · the smallest realistic shape of a zfb
            site.
          </p>
        </footer>
      </body>
    </html>
  );
}
