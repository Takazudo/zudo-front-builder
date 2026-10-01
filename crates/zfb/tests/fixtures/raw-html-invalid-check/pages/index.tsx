export default function InvalidRawTextPage() {
  return (
    <html lang="en">
      <head>
        <title>Invalid raw-text child</title>
        <script rawHtml={`window.__zfbInvalid = true;`}>unexpected child</script>
      </head>
      <body>zfb check must reject script children</body>
    </html>
  );
}
