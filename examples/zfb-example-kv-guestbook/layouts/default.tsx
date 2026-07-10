import type { ComponentChildren } from "preact";

import "../styles/global.css";

const CRITICAL_CSS = `
:root {
  color-scheme: light;
  --bg: #f8faf7;
  --panel: #ffffff;
  --text: #17201a;
  --muted: #5f6f65;
  --border: #d9e2da;
  --accent: #2563eb;
  --accent-strong: #1746a2;
  --success: #157f3b;
  --warning: #9a5b00;
  --danger: #b42318;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  line-height: 1.5;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
}

a {
  color: var(--accent);
}

.shell {
  width: min(100% - 32px, 760px);
  margin: 0 auto;
  padding: 32px 0 48px;
}

.site-header,
.site-footer {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  align-items: center;
  border-block: 1px solid var(--border);
  padding: 14px 0;
}

.site-header {
  border-block-start: 0;
  margin-block-end: 28px;
}

.site-footer {
  border-block-end: 0;
  margin-block-start: 36px;
  color: var(--muted);
  font-size: 0.875rem;
}

.brand {
  color: var(--text);
  font-weight: 700;
  text-decoration: none;
}

.muted {
  color: var(--muted);
}

.stack {
  display: grid;
  gap: 20px;
}

.panel {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 18px;
}

.page-title {
  margin: 0 0 8px;
  font-size: clamp(1.75rem, 4vw, 2.35rem);
  line-height: 1.1;
}

.section-title {
  margin: 0 0 12px;
  font-size: 1rem;
}

.entry-form {
  display: grid;
  gap: 10px;
}

label {
  font-weight: 650;
}

textarea {
  width: 100%;
  min-height: 112px;
  resize: vertical;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 10px 12px;
  color: var(--text);
  background: #ffffff;
  font: inherit;
}

textarea:focus {
  outline: 3px solid #bfd7ff;
  border-color: var(--accent);
}

.form-row,
.entry-meta,
.api-list {
  display: flex;
  flex-wrap: wrap;
  gap: 10px 14px;
  align-items: center;
}

.form-row {
  justify-content: space-between;
}

button {
  border: 0;
  border-radius: 6px;
  padding: 9px 14px;
  background: var(--accent);
  color: #ffffff;
  font: inherit;
  font-weight: 700;
  cursor: pointer;
}

button:hover {
  background: var(--accent-strong);
}

.notice,
.error {
  border-radius: 6px;
  padding: 10px 12px;
  border: 1px solid;
}

.notice {
  color: var(--success);
  border-color: #b8dec5;
  background: #effaf2;
}

.error {
  color: var(--danger);
  border-color: #f2b8b5;
  background: #fff1f0;
}

.entry-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: grid;
  gap: 12px;
}

.entry {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px;
  background: #ffffff;
}

.entry p {
  margin: 0 0 10px;
  white-space: pre-wrap;
}

.entry-meta {
  color: var(--muted);
  font-size: 0.8125rem;
}

code {
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 2px 6px;
  background: #f3f6f3;
  color: #26312a;
  font-size: 0.875em;
}

@media (max-width: 560px) {
  .shell {
    width: min(100% - 24px, 760px);
    padding-block-start: 20px;
  }

  .site-header,
  .site-footer,
  .form-row {
    align-items: flex-start;
    flex-direction: column;
  }

  button {
    width: 100%;
  }
}
`;

type Props = {
  title?: string;
  children: ComponentChildren;
};

export default function DefaultLayout({ title = "zfb KV guestbook", children }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{title}</title>
        <style>{CRITICAL_CSS}</style>
      </head>
      <body>
        <div class="shell">
          <header class="site-header">
            <a class="brand" href="/">
              zfb KV guestbook
            </a>
            <span class="muted">Cloudflare Workers KV</span>
          </header>
          <main>{children}</main>
          <footer class="site-footer">
            <span>zfb example</span>
            <a href="/api/entries">JSON</a>
          </footer>
        </div>
      </body>
    </html>
  );
}
