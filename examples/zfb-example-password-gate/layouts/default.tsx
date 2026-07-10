import type { ComponentChildren } from "preact";

import "../styles/global.css";

type NavKey = "overview" | "updates" | "checklist";

type Props = {
  title: string;
  active: NavKey;
  children: ComponentChildren;
};

const navItems: Array<{ key: NavKey; label: string; href: string }> = [
  { key: "overview", label: "Overview", href: "/" },
  { key: "updates", label: "Updates", href: "/updates/" },
  { key: "checklist", label: "Checklist", href: "/checklist/" },
];

export default function DefaultLayout({ title, active, children }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="robots" content="noindex" />
        <title>{title}</title>
      </head>
      <body>
        <div class="site-shell">
          <header class="site-header">
            <a class="site-title" href="/">
              Preview Desk
            </a>
            <nav class="site-nav" aria-label="Preview sections">
              {navItems.map((item) => (
                <a
                  key={item.key}
                  href={item.href}
                  aria-current={active === item.key ? "page" : undefined}
                >
                  {item.label}
                </a>
              ))}
            </nav>
          </header>
          <main>{children}</main>
        </div>
      </body>
    </html>
  );
}
