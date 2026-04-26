// Lightweight ambient JSX types so the public Island wrapper does not
// hard-depend on either preact or react as a runtime/types dependency.
//
// Both Preact and React's `children` accept this minimal union; the build
// is type-erased so the actual rendering happens in the user's app under
// whichever framework adapter they chose.

export type ReactNode =
  | string
  | number
  | boolean
  | null
  | undefined
  | bigint
  | ReactNodeArray
  | { readonly [key: string]: unknown };

export interface ReactNodeArray extends ReadonlyArray<ReactNode> {}
