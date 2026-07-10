import "../styles/global.css";

const checks = [
  {
    href: "/proxy/anything/reverse-proxy?via=zfb",
    label: "Normal proxy",
    detail: "Streams a JSON response from the upstream /anything endpoint.",
  },
  {
    href: "/proxy/redirect-to?url=/anything/redirect-target&status_code=302",
    label: "Redirect rewrite",
    detail: "Returns an upstream Location header rewritten back under /proxy/.",
  },
  {
    href: "/proxy/cookies/set?zfb_proxy_cookie=demo",
    label: "Cookie strip",
    detail: "Shows an upstream Set-Cookie response without forwarding the cookie.",
  },
  {
    href: "/proxy/response-headers?Content-Security-Policy=default-src%20%27self%27&Strict-Transport-Security=max-age%3D31536000&X-Demo=kept",
    label: "Header policy strip",
    detail: "Keeps ordinary headers while removing upstream CSP and HSTS.",
  },
];

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>zfb reverse proxy example</title>
      </head>
      <body>
        <main class="shell">
          <header class="page-header">
            <p class="eyebrow">Cloudflare SSR route</p>
            <h1>Reverse proxy under /proxy/</h1>
            <p>
              A compact zfb example that forwards requests to a deterministic httpbin-compatible
              upstream while keeping proxy-specific headers under control.
            </p>
          </header>

          <section class="status-grid" aria-label="Proxy settings">
            <div>
              <span>Origin</span>
              <strong>https://httpbingo.org</strong>
            </div>
            <div>
              <span>Route</span>
              <strong>pages/proxy/[...path].tsx</strong>
            </div>
            <div>
              <span>Runtime</span>
              <strong>Cloudflare Worker</strong>
            </div>
          </section>

          <section class="check-list" aria-label="Proxy checks">
            {checks.map((check) => (
              <a class="check-row" href={check.href} key={check.href}>
                <span>
                  <strong>{check.label}</strong>
                  <small>{check.detail}</small>
                </span>
                <code>{check.href}</code>
              </a>
            ))}
          </section>
        </main>
      </body>
    </html>
  );
}
