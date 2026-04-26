import type { ComponentChildren } from "preact";

import "../styles/global.css";

type Props = {
  title?: string;
  children: ComponentChildren;
};

export default function DefaultLayout({ title = "zfb site", children }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{title}</title>
      </head>
      <body>{children}</body>
    </html>
  );
}
