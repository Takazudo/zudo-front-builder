// Stub blog post data. Used by:
//   - pages/blog/index.tsx (lists all posts)
//   - pages/blog/[slug].tsx (paths() over this list)
//   - pages/blog/page/[page].tsx (paginated view)
//
// Kept tiny on purpose: enough to multiply over for paths()
// and enough to paginate cleanly.

export type Post = {
  slug: string;
  title: string;
  body: string;
};

export const posts: Post[] = [
  { slug: "hello", title: "Hello", body: "First post." },
  { slug: "second", title: "Second", body: "Second post." },
  { slug: "third", title: "Third", body: "Third post." },
  { slug: "fourth", title: "Fourth", body: "Fourth post." },
];
