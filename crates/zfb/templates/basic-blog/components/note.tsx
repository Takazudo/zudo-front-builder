import type { ComponentChildren } from "preact";

type Props = {
  /**
   * Optional heading rendered above the body. When omitted, the admonition
   * shows only its body — useful for inline callouts that don't need a
   * label.
   */
  title?: string;
  children: ComponentChildren;
};

/**
 * Minimal admonition component used by `content/blog/hello-zfb.mdx`.
 *
 * The point of this component in the basic-blog example is **not** to be a
 * pretty admonition — it's to prove that custom JSX in MDX content reaches
 * the page through the `components` prop on `<post.Content />`. The MDX
 * post writes `<Note title="...">…</Note>` and the per-post route in
 * `pages/blog/[slug].tsx` passes `Note` in alongside `defaultComponents`.
 *
 * Implementation is a thin passthrough: a `<aside>` wrapper with a
 * `data-component="note"` hook for styling and an optional `<strong>` title
 * line above the children. Visual treatment lives in `styles/global.css`
 * (or is left to the reader — the example ships unstyled on purpose so the
 * delivery contract is what's visible, not the CSS).
 */
export default function Note({ title, children }: Props) {
  return (
    <aside class="admonition admonition--note" data-component="note" role="note">
      {title ? <strong class="admonition__title">{title}</strong> : null}
      <div class="admonition__body">{children}</div>
    </aside>
  );
}
