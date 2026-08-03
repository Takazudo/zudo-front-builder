import type { ComponentChildren } from "preact";
import { Island } from "@takazudo/zfb";

import ThemeToggle from "~/components/theme-toggle";
import "~/styles/global.css";

/**
 * Inline pre-hydration script. Runs synchronously before the page paints,
 * reads the persisted theme preference (or `prefers-color-scheme`), and
 * sets `document.documentElement.dataset.theme` so the stylesheet picks
 * the correct palette on the very first frame. Must stay tiny, SSR-safe,
 * and self-contained — bundlers may not see this string.
 */
const THEME_BOOTSTRAP_SCRIPT = `(() => {
  try {
    var saved = localStorage.getItem("basic-blog:theme");
    var hasMM = typeof window !== "undefined" && typeof window.matchMedia === "function";
    var theme = saved === "light" || saved === "dark"
      ? saved
      : (hasMM && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    document.documentElement.dataset.theme = theme;
  } catch (e) {
    document.documentElement.dataset.theme = "light";
  }
})();`;

type Props = {
  title?: string;
  description?: string;
  children: ComponentChildren;
};

const NAV = [
  { href: "/#posts", label: "Blog" },
  { href: "/about", label: "About" },
];

/**
 * The single shared chrome for every page in the example. The `"use client"`
 * boundary is at `components/theme-toggle.tsx`, so this layout itself stays
 * a plain server component — only the toggle ships JS to the browser.
 */
export default function DefaultLayout({
  title = "basic-blog · a zfb starter",
  description,
  children,
}: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        {/* Inline data-URI favicon: no public/ dir needed, and it stops the
            browser's implicit /favicon.ico request from 404-ing the console. */}
        <link
          rel="icon"
          href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Crect width='16' height='16' rx='3' fill='%232563eb'/%3E%3Ctext x='8' y='12' font-family='ui-monospace,monospace' font-size='10' fill='white' text-anchor='middle'%3Ez%3C/text%3E%3C/svg%3E"
        />
        <title>{title}</title>
        {description ? <meta name="description" content={description} /> : null}
        {/* Apply theme before paint to avoid FOUC. See script doc above. */}
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOTSTRAP_SCRIPT }} />
      </head>
      <body class="bg-white text-neutral-700 antialiased dark:bg-neutral-950 dark:text-neutral-300">
        <div class="mx-auto flex min-h-screen max-w-2xl flex-col px-5 sm:px-6">
          <header class="flex items-center justify-between gap-4 border-b border-neutral-200 py-5 dark:border-neutral-800">
            <a href="/" class="font-semibold tracking-tight text-neutral-900 dark:text-neutral-100">
              basic-blog
            </a>
            <nav class="flex items-center gap-5 text-sm">
              {NAV.map((item) => (
                <a
                  key={item.href}
                  href={item.href}
                  class="text-neutral-600 transition-colors hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-neutral-100"
                >
                  {item.label}
                </a>
              ))}
              {/*
                The toggle is a `"use client"` island. Wrapping in `<Island>`
                is what marks it for the zfb hydration pipeline and picks the
                scheduling strategy. `"idle"` keeps the toggle passive until
                the browser has spare cycles — chrome doesn't need to be
                interactive on the very first frame.
              */}
              <Island when="idle">
                <ThemeToggle />
              </Island>
            </nav>
          </header>
          <main class="grow py-12">{children}</main>
          <footer class="border-t border-neutral-200 py-6 text-sm text-neutral-500 dark:border-neutral-800 dark:text-neutral-500">
            <p>
              Built with{" "}
              <a href="https://zfb.takazudomodular.com" class="text-accent hover:underline">
                zfb
              </a>
              . Edit this footer in <code>layouts/default.tsx</code>.
            </p>
          </footer>
        </div>
      </body>
    </html>
  );
}
