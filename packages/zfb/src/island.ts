// Build-time Island wrapper for the owned zudo-react runtime.
import { ownedIslandBoundary } from "./island-boundary.js";
import type { Child, Description } from "./zudo-react/description.js";
import { DEFAULT_WHEN, resolveWhen, type When } from "./types.js";

export { resolveWhen } from "./types.js";
export const HYDRATE_MARKER_ATTR = "data-zfb-island";
export const SKIP_SSR_MARKER_ATTR = "data-zfb-island-skip-ssr";

export interface IslandProps {
  when?: When;
  media?: string;
  ssrFallback?: Child;
  /** Stable key for preserving this island root across client-side navigation. */
  persist?: string;
  /** Keep the original props and state when the key matches (default: refresh changed props). */
  persistProps?: boolean;
  /** JSX erases the tag kind; the boundary checks for one function component at runtime. */
  children?: Description;
}

export type IslandElement = Description;

export function Island(props: IslandProps): IslandElement {
  const { when, media } = resolveMediaProps(props);
  return ownedIslandBoundary(
    props.children,
    props.ssrFallback,
    when,
    media,
    props.persist,
    props.persistProps,
  );
}

function resolveMediaProps(props: IslandProps): { when: When; media: string | undefined } {
  const when = resolveWhen(props.when);
  const media = props.media;

  if (when === "media" && !media) {
    if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
      // eslint-disable-next-line no-console
      console.warn(
        `[zfb] <Island when="media"> requires the \`media\` prop (a CSS media query string). ` +
          `Falling back to "${DEFAULT_WHEN}".`,
      );
    }
    return { when: DEFAULT_WHEN, media: undefined };
  }

  if (media && when !== "media") {
    if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
      // eslint-disable-next-line no-console
      console.warn(
        `[zfb] The \`media\` prop is only used when \`when="media"\`. ` +
          `Current when="${when}" — \`media\` will be ignored.`,
      );
    }
    return { when, media: undefined };
  }

  return { when, media };
}
