export const prerender = false;

export default function InvalidRawTextPage() {
  const untypedProps: any = {
    rawHtml: `window.__zfbWindInvalid = "untyped";`,
    children: "this child must be rejected",
  };

  return (
    <html lang="en">
      <head>
        <title>Wind invalid raw HTML SSR route</title>
        <style {...untypedProps} />
      </head>
      <body>WIND_INVALID_ROUTE</body>
    </html>
  );
}
