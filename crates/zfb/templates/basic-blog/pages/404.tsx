import DefaultLayout from "~/layouts/default";

/**
 * Error-page convention: a top-level `404.tsx` emits a flat `dist/404.html`
 * rather than `dist/404/index.html`, which is the filename `zfb preview` —
 * and most static hosts — serve for an unmatched request.
 */
export default function NotFoundPage() {
  return (
    <DefaultLayout title="404 · basic-blog">
      <div class="py-16 text-center">
        <p class="font-mono text-sm tracking-[0.2em] text-neutral-400 dark:text-neutral-600">404</p>
        <h1 class="mt-4 text-2xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-50">
          This page doesn't exist
        </h1>
        <p class="mt-3 text-neutral-600 dark:text-neutral-400">
          The link may be out of date, or the post may have been renamed.
        </p>
        <a
          href="/"
          class="mt-8 inline-block rounded-md border border-neutral-200 px-4 py-2 text-sm transition-colors hover:border-neutral-300 hover:text-neutral-900 dark:border-neutral-800 dark:hover:border-neutral-700 dark:hover:text-neutral-100"
        >
          Back home
        </a>
      </div>
    </DefaultLayout>
  );
}
