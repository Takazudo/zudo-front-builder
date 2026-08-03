import type { ComponentChildren } from "preact";

import DefaultLayout from "~/layouts/default";

/**
 * A static route: no collection, no `paths()`, no props. A default-exported
 * component under `pages/` is all a page needs — this file becomes
 * `/about/index.html`.
 */
const STRUCTURE: { path: string; what: ComponentChildren }[] = [
  {
    path: "pages/",
    what: (
      <>
        Routes. One file per URL; <code>[slug].tsx</code> expands via its <code>paths()</code>{" "}
        export.
      </>
    ),
  },
  { path: "layouts/", what: "Shared HTML shell — <head>, header, footer, the theme bootstrap." },
  {
    path: "components/",
    what: (
      <>
        Plain components, plus the one <code>"use client"</code> island.
      </>
    ),
  },
  {
    path: "content/blog/",
    what: (
      <>
        The <code>blog</code> collection. Markdown and MDX files with frontmatter.
      </>
    ),
  },
  {
    path: "styles/",
    what: (
      <>
        Tailwind entry, theme tokens, and the <code>.prose</code> markdown styles.
      </>
    ),
  },
  { path: "lib/", what: "Types shared between routes." },
  {
    path: "mdx-components.tsx",
    what: "Project-wide component map applied to every rendered entry.",
  },
  { path: "zfb.config.ts", what: "Framework, collections + schema, and the markdown features." },
];

export default function AboutPage() {
  return (
    <DefaultLayout
      title="About · basic-blog"
      description="What each directory in this zfb starter is for."
    >
      <h1 class="text-2xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
        About this starter
      </h1>
      <p class="mt-4 leading-relaxed">
        This site is the <code class="text-sm">basic-blog</code> template that ships with{" "}
        <a href="https://zfb.takazudomodular.com" class="text-accent hover:underline">
          zfb
        </a>
        . Pages are server-rendered to static HTML at build time; the only JavaScript that reaches
        the browser is the theme toggle in the header, because it is the only component marked{" "}
        <code class="text-sm">"use client"</code>.
      </p>

      <h2 class="mt-12 text-xs font-semibold tracking-[0.08em] text-neutral-500 uppercase">
        Where things live
      </h2>
      <dl class="mt-6 space-y-4">
        {STRUCTURE.map((row) => (
          <div key={row.path} class="sm:flex sm:gap-6">
            <dt class="font-mono text-sm text-neutral-900 sm:w-48 sm:shrink-0 dark:text-neutral-100">
              {row.path}
            </dt>
            <dd class="mt-1 text-sm leading-relaxed text-neutral-600 sm:mt-0 dark:text-neutral-400">
              {row.what}
            </dd>
          </div>
        ))}
      </dl>

      <h2 class="mt-12 text-xs font-semibold tracking-[0.08em] text-neutral-500 uppercase">
        Commands
      </h2>
      <ul class="mt-6 space-y-2 text-sm">
        <li>
          <code>zfb dev</code> — dev server with live reload
        </li>
        <li>
          <code>zfb build</code> — static build into <code>dist/</code>
        </li>
        <li>
          <code>zfb preview</code> — serve the built output
        </li>
        <li>
          <code>zfb check</code> — TypeScript + content-schema check
        </li>
      </ul>
    </DefaultLayout>
  );
}
