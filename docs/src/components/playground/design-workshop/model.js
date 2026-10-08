/**
 * Shared project-owned design decisions and deterministic export boundary.
 * Browser previews, downloads and starter generation consume this module;
 * it imports no DOM, filesystem, host theme or compiler APIs.
 */
const PRESETS = {
  everyday: {
    name: "Everyday",
    tag: "Suggested start",
    description: "Clear hierarchy. Comfortable defaults.",
    detail: "A calm starting point for blogs, documentation and small product sites.",
    traits: ["Sans headings", "Open spacing", "Soft corners"],
    values: {
      accent: "#3159d8",
      background: "#f4f7fb",
      surface: "#ffffff",
      ink: "#17253c",
      muted: "#586882",
      border: "#d5deeb",
      headingFont: "sans",
      bodySize: 16,
      lineHeight: 1.65,
      groupGap: 16,
      sectionGap: 40,
      horizontalSpace: 24,
      horizontalGap: 16,
      radius: 8,
      readingWidth: 60,
      headingSize: 42,
    },
    rules: [
      "Use accent for the primary action and selected states. Keep ordinary text neutral.",
      "Use one corner radius for cards, fields and buttons. Avoid decorative shadows.",
      "Use a larger gap between sections than between related items.",
      "Keep the body text measure constrained, even on wide screens.",
    ],
  },
  editorial: {
    name: "Editorial",
    tag: "For reading",
    description: "Quiet surfaces. Expressive typography.",
    detail: "A reading-first seed for journals, essays and independent publications.",
    traits: ["Serif headings", "Generous rhythm", "Square edges"],
    values: {
      accent: "#913b28",
      background: "#faf8f2",
      surface: "#fffefa",
      ink: "#352b26",
      muted: "#736457",
      border: "#d9cdbd",
      headingFont: "serif",
      bodySize: 17,
      lineHeight: 1.8,
      groupGap: 20,
      sectionGap: 56,
      horizontalSpace: 30,
      horizontalGap: 20,
      radius: 0,
      readingWidth: 56,
      headingSize: 47,
    },
    rules: [
      "Let headings carry the voice; keep body text easy to read in both languages.",
      "Prefer flat surfaces, square corners and quiet dividing lines.",
      "Reserve accent for the primary action and selected states.",
      "Give each section room to breathe; keep the reading column narrow.",
    ],
  },
  workbench: {
    name: "Workbench",
    tag: "For useful tools",
    description: "Compact structure. Precise emphasis.",
    detail: "A compact seed for developer tools, technical notes and dense interfaces.",
    traits: ["Mono headings", "Compact rhythm", "Dark surfaces"],
    values: {
      accent: "#cae887",
      background: "#171e1b",
      surface: "#202a25",
      ink: "#e0e9df",
      muted: "#acbaac",
      border: "#405047",
      headingFont: "mono",
      bodySize: 16,
      lineHeight: 1.55,
      groupGap: 12,
      sectionGap: 32,
      horizontalSpace: 18,
      horizontalGap: 12,
      radius: 3,
      readingWidth: 64,
      headingSize: 35,
    },
    rules: [
      "Use dark surfaces and neutral text; reserve the light accent for actions.",
      "Use monospace for headings and labels, with sans-serif body text.",
      "Keep the layout compact without shrinking body text.",
      "Use small corners and fine borders to separate working areas.",
    ],
  },
};
const FONTS = {
  sans: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
  serif: 'Georgia, "Times New Roman", "Yu Mincho", "Hiragino Mincho ProN", serif',
  mono: 'ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace',
};
const KEYS = [
  "accent",
  "headingFont",
  "bodySize",
  "lineHeight",
  "groupGap",
  "sectionGap",
  "horizontalSpace",
  "radius",
  "readingWidth",
];
const HEX = /^#[a-fA-F0-9]{6}$/;
const LIMITS = {
  bodySize: [14, 20],
  lineHeight: [1.4, 2],
  groupGap: [8, 24],
  sectionGap: [16, 64],
  horizontalSpace: [12, 36],
  radius: [0, 18],
  readingWidth: [42, 72],
};
function luminance(hex) {
  if (typeof hex !== "string" || !HEX.test(hex))
    throw new TypeError("Expected a six-digit hex color");
  const c = hex
    .slice(1)
    .match(/../g)
    .map((x) => parseInt(x, 16) / 255)
    .map((x) => (x <= 0.04045 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4)));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}
function contrast(a, b) {
  const x = luminance(a),
    y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}
function onAccent(v) {
  return contrast(v.accent, "#ffffff") >= contrast(v.accent, "#14201a") ? "#ffffff" : "#14201a";
}
function variableMap(v) {
  validateValues(v);
  return {
    "--ds-brand": v.accent,
    "--ds-accent": "var(--ds-brand)",
    "--ds-on-accent": onAccent(v),
    "--ds-background": v.background,
    "--ds-surface": v.surface,
    "--ds-ink": v.ink,
    "--ds-muted": v.muted,
    "--ds-border": v.border,
    "--ds-type-base": v.bodySize + "px",
    "--ds-font-body": "var(--ds-type-base)",
    "--ds-font-heading": v.headingSize + "px",
    "--ds-font-section": Math.round(v.headingSize * 0.61) + "px",
    "--ds-font-caption": "0.8125rem",
    "--ds-leading-body": String(v.lineHeight),
    "--ds-font-ui": FONTS.sans,
    "--ds-font-display": FONTS[v.headingFont],
    "--ds-vsp-stack": v.groupGap + "px",
    "--ds-vsp-section": v.sectionGap + "px",
    "--ds-hsp-gutter": v.horizontalGap + "px",
    "--ds-hsp-card": v.horizontalSpace + "px",
    "--ds-hsp-page": v.horizontalSpace + 8 + "px",
    "--ds-radius-panel": v.radius + "px",
    "--ds-size-reading": v.readingWidth + "ch",
  };
}
function variablesCSS(v) {
  return (
    ":root {\n" +
    Object.entries(variableMap(v))
      .map(([k, val]) => "  " + k + ": " + val + ";")
      .join("\n") +
    "\n}\n"
  );
}
function currentRules(state) {
  const v = state.values;
  validateValues(v);
  return [
    `Use accent for primary actions, selected states and keyboard focus. Keep text, notice borders and supporting content neutral.`,
    `Use ${v.radius === 0 ? "square corners" : v.radius + "px corners"} for cards, fields and buttons. Keep surfaces flat.`,
    `Use ${v.groupGap}px within vertical groups and ${v.sectionGap}px between sections. Horizontal card padding is ${v.horizontalSpace}px; column gutters are independently ${v.horizontalGap}px.`,
    `Use ${v.bodySize}px body text with ${v.lineHeight} line height and a reading measure of ${v.readingWidth}ch. Headings use ${v.headingFont}; the body uses a clear sans-serif.`,
  ];
}
const PREVIEW_CSS = `
*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:var(--ds-background);color:var(--ds-ink);font-family:var(--ds-font-ui);font-size:var(--ds-font-body);line-height:var(--ds-leading-body)}button,input,select{font:inherit}button,a,summary{cursor:pointer}button{color:inherit}a{color:var(--ds-ink);text-decoration-thickness:2px;text-decoration-color:var(--ds-ink);text-underline-offset:5px}button:focus-visible,a:focus-visible,input:focus-visible,summary:focus-visible{outline:3px solid var(--ds-accent);outline-offset:3px}[hidden]{display:none!important}h1,h2,h3,p{margin:0}.sample-header{padding:18px var(--ds-hsp-page);display:flex;justify-content:space-between;align-items:center;gap:16px;border-bottom:1px solid var(--ds-border);background:var(--ds-surface)}.sample-logo{font-family:var(--ds-font-display);font-size:13px;font-weight:700;letter-spacing:1.5px;text-transform:uppercase;text-decoration:none;color:var(--ds-ink)}.sample-nav{display:flex;gap:18px;align-items:center}.sample-nav a,.sample-nav button{font-size:12px;text-decoration:none;background:none;border:0;padding:0;color:var(--ds-muted)}.sample-nav a.active{color:var(--ds-ink);text-decoration:underline;text-decoration-color:var(--ds-accent)}.page-wrap{padding:var(--ds-vsp-section) var(--ds-hsp-page);max-width:1100px;margin:auto}.hero{max-width:var(--ds-size-reading)}.kicker{font-size:11px;text-transform:uppercase;letter-spacing:1.6px;line-height:1.5;margin-bottom:var(--ds-vsp-stack);color:var(--ds-muted);font-family:var(--ds-font-display)}h1{font-family:var(--ds-font-display);font-size:var(--ds-font-heading);font-weight:600;letter-spacing:-1.5px;line-height:1.22;margin-bottom:var(--ds-vsp-stack)}h2{font-family:var(--ds-font-display);font-size:var(--ds-font-section);line-height:1.3;letter-spacing:-.5px;font-weight:550}h3{font-family:var(--ds-font-display);font-size:1.125rem;font-weight:600;line-height:1.5}.intro{max-width:var(--ds-size-reading);color:var(--ds-muted);font-size:var(--ds-font-body);line-height:var(--ds-leading-body)}.hero-actions{display:flex;align-items:center;column-gap:var(--ds-hsp-gutter);row-gap:var(--ds-vsp-stack);margin-top:var(--ds-vsp-stack)}.ds-button{white-space:nowrap;display:inline-flex;align-items:center;justify-content:center;padding:10px 17px;min-height:40px;border-radius:var(--ds-radius-panel);border:1px solid var(--ds-accent);background:var(--ds-accent);color:var(--ds-on-accent);font-size:14px;font-weight:600;text-decoration:none;line-height:1.4;gap:7px}.ds-button.secondary{background:var(--ds-surface);border-color:var(--ds-border);color:var(--ds-ink)}.ds-button:hover{filter:brightness(.95)}.ds-button:disabled{opacity:.5;cursor:not-allowed}.text-link{font-size:13px}.section{margin-top:var(--ds-vsp-section)}.section-title{display:flex;align-items:center;justify-content:space-between;column-gap:var(--ds-hsp-gutter);row-gap:var(--ds-vsp-stack);margin-bottom:var(--ds-vsp-stack)}.section-title span{font-size:12px;color:var(--ds-muted)}.cards{display:grid;grid-template-columns:1fr 1fr;row-gap:var(--ds-vsp-stack);column-gap:var(--ds-hsp-gutter)}.ds-card{padding:var(--ds-vsp-stack) var(--ds-hsp-card);border:1px solid var(--ds-border);background:var(--ds-surface);border-radius:var(--ds-radius-panel)}.card-meta{font-size:11px;color:var(--ds-muted);margin-bottom:calc(var(--ds-vsp-stack) * .6);display:flex;gap:10px;flex-wrap:wrap}.ds-card h3{margin-bottom:calc(var(--ds-vsp-stack) * .6)}.ds-card p{font-size:var(--ds-font-body);color:var(--ds-muted);line-height:var(--ds-leading-body)}.card-foot{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:var(--ds-vsp-stack)}.card-foot span{font-size:11px;color:var(--ds-muted)}.bookmark{font-size:12px;background:transparent;border:1px solid var(--ds-border);padding:5px 9px;border-radius:var(--ds-radius-panel);color:var(--ds-ink)}.bookmark[aria-pressed=true]{background:var(--ds-accent);color:var(--ds-on-accent);border-color:var(--ds-accent)}.design-note{padding:14px 17px;margin-top:var(--ds-vsp-stack);border-left:3px solid var(--ds-border);background:var(--ds-surface);border-top:1px solid var(--ds-border);border-right:1px solid var(--ds-border);border-bottom:1px solid var(--ds-border);font-size:13px;line-height:1.8;color:var(--ds-muted)}.design-note b{color:var(--ds-ink);font-size:12px;display:block;font-weight:650;margin-bottom:4px}.design-note code{font-size:11px;color:var(--ds-ink);white-space:normal}.subscribe{display:grid;grid-template-columns:1fr 1fr;column-gap:var(--ds-hsp-gutter);row-gap:var(--ds-vsp-stack);align-items:center;background:var(--ds-surface);border:1px solid var(--ds-border);padding:var(--ds-vsp-stack) var(--ds-hsp-card);border-radius:var(--ds-radius-panel)}.subscribe p{font-size:var(--ds-font-body);color:var(--ds-muted);margin-top:8px}.form-fields{display:flex;flex-direction:column;gap:10px}.form-fields label{font-size:12px;color:var(--ds-muted)}.input-row{display:flex;gap:8px}.ds-input{display:block;width:100%;min-width:0;padding:10px 12px;border:1px solid var(--ds-border);border-radius:var(--ds-radius-panel);background:var(--ds-background);color:var(--ds-ink);font-size:var(--ds-font-body);line-height:var(--ds-leading-body)}.ds-input::placeholder{color:var(--ds-muted);opacity:.8}.form-note{font-size:11px;color:var(--ds-muted)}.reading-sample{max-width:var(--ds-size-reading);padding-top:var(--ds-vsp-section);border-top:1px solid var(--ds-border)}.reading-sample h2{margin-bottom:var(--ds-vsp-stack)}.reading-sample p{margin:var(--ds-vsp-stack) 0;color:var(--ds-muted)}.reading-sample code{font-size:13px;color:var(--ds-ink)}.sample-footer{margin-top:var(--ds-vsp-section);padding-top:var(--ds-vsp-stack);border-top:1px solid var(--ds-border);display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap;font-size:11px;color:var(--ds-muted)}.about-panel{margin-top:var(--ds-vsp-stack);padding:var(--ds-vsp-stack);border:1px solid var(--ds-border);border-radius:var(--ds-radius-panel);color:var(--ds-muted);font-size:14px}.specimen-header{padding-bottom:var(--ds-vsp-stack);border-bottom:1px solid var(--ds-border);margin-bottom:var(--ds-vsp-section)}.specimen-header h1{font-size:var(--ds-font-section);letter-spacing:-.7px;margin-bottom:8px}.specimen-header p{font-size:14px;color:var(--ds-muted)}.specimen{margin-top:var(--ds-vsp-section)}.spec-label{font-size:11px;font-family:ui-monospace,monospace;letter-spacing:1.1px;text-transform:uppercase;color:var(--ds-muted);margin-bottom:var(--ds-vsp-stack)}.button-sample{display:flex;gap:12px;align-items:center;flex-wrap:wrap}.field-example{max-width:400px;display:grid;gap:10px}.field-example label{font-size:13px}.message{padding:var(--ds-vsp-stack) var(--ds-hsp-card);border:1px solid var(--ds-border);border-left:3px solid var(--ds-border);border-radius:var(--ds-radius-panel);background:var(--ds-surface);font-size:14px}.message b{display:block;color:var(--ds-ink);margin-bottom:6px}.message p{color:var(--ds-muted);font-size:var(--ds-font-body);line-height:var(--ds-leading-body)}.token-colors{display:grid;grid-template-columns:repeat(5,1fr);gap:10px}.color-sample{min-width:0}.color-sample .swatch{height:48px;border:1px solid var(--ds-border);border-radius:var(--ds-radius-panel);margin-bottom:7px}.color-sample b{font-size:11px;display:block}.color-sample code{display:block;font-size:9px;color:var(--ds-muted);word-break:break-all}.type-sample{padding:var(--ds-vsp-stack) 0;border-top:1px solid var(--ds-border);display:grid;grid-template-columns:95px 1fr;gap:16px;align-items:baseline}.type-sample label{font-family:ui-monospace,monospace;font-size:11px;color:var(--ds-muted)}.type-sample h2{font-size:var(--ds-font-heading)}.type-sample p{font-size:var(--ds-font-body)}.type-sample small{font-size:var(--ds-font-caption);color:var(--ds-muted)}.spacing-sample{display:grid;gap:15px}.spacing-row{display:grid;grid-template-columns:140px 1fr;gap:18px;align-items:center}.spacing-row code{font-size:11px;color:var(--ds-muted)}.space-bar{background:var(--ds-accent);height:12px;min-width:4px}.group-example{border:1px solid var(--ds-border);border-radius:var(--ds-radius-panel);padding:var(--ds-vsp-stack) var(--ds-hsp-card);margin-top:var(--ds-vsp-stack);background:var(--ds-surface)}.group-example .example-group{display:grid;gap:var(--ds-vsp-stack)}.example-group+ .example-group{margin-top:var(--ds-vsp-section)}.example-group div{height:8px;background:var(--ds-border);border-radius:2px;width:80%}.example-group div:first-child{width:55%;background:var(--ds-muted)}.inspect-on [data-inspect]{outline:1px dashed var(--ds-accent);outline-offset:4px;cursor:crosshair;position:relative}.inspect-on [data-inspect]:hover{outline-width:3px}.inspect-on .ds-button{outline-offset:5px}.sample-toast{position:fixed;bottom:14px;left:50%;transform:translateX(-50%);background:var(--ds-ink);color:var(--ds-surface);padding:10px 16px;border-radius:var(--ds-radius-panel);font-size:12px;z-index:50;max-width:90%;text-align:center}.reading-note-title{font-size:11px;font-weight:700;letter-spacing:1px;color:var(--ds-muted);margin-bottom:var(--ds-vsp-stack)}.state-label{display:inline-block;padding:4px 8px;border:1px solid var(--ds-border);border-radius:var(--ds-radius-panel);font-size:12px;color:var(--ds-muted)}
@media(max-width:600px){.sample-header{padding:17px max(18px,calc(var(--ds-hsp-page)*.7))}.page-wrap{padding:var(--ds-vsp-section) max(18px,calc(var(--ds-hsp-page)*.7))}h1{font-size:clamp(28px,8vw,var(--ds-font-heading));letter-spacing:-1px}.cards{grid-template-columns:1fr}.hero-actions{flex-wrap:wrap}.section-title{align-items:baseline}.section-title span{font-size:10px}.subscribe{grid-template-columns:1fr}.sample-nav{gap:13px}.sample-logo{font-size:11px}.sample-nav a,.sample-nav button{font-size:11px}.token-colors{grid-template-columns:repeat(3,1fr)}.type-sample{grid-template-columns:1fr;gap:8px}.type-sample h2{font-size:clamp(26px,8vw,var(--ds-font-heading))}.spacing-row{grid-template-columns:120px 1fr;gap:12px}.ds-card h3{font-size:18px}.ds-card p{font-size:var(--ds-font-body)}.sample-footer{font-size:11px}.input-row{flex-wrap:wrap}.input-row .ds-button{width:100%}}
@media(prefers-reduced-motion:reduce){html{scroll-behavior:auto}}
`;
const NOTE = (title, body) => `<aside class="design-note"><b>${title}</b>${body}</aside>`;
function pageMarkup(state, notes = false) {
  const v = state.values;
  validateValues(v);
  return `<header class="sample-header"><a class="sample-logo" href="#top">Fieldnotes</a><nav class="sample-nav" aria-label="Example site"><a class="active" href="#journal">Journal</a><button type="button" data-demo="about">About</button></nav></header><main class="page-wrap" id="top"><section class="hero"><p class="kicker">Notes on making things</p><h1>Small ideas.<br>Room to grow.</h1><p class="intro" data-inspect="body">つくること、考えること。その途中にある小さな発見を。<br>A journal of experiments, useful details, and work in progress.</p><div class="hero-actions"><a class="ds-button" data-inspect="accent" href="#journal">Read the journal</a><a class="text-link" href="#how-it-works">How this site is styled</a></div><div class="about-panel" id="about-panel" hidden>Fieldnotes is an example starter site for collecting ideas and sharing work. Replace this introduction with your own story.</div></section>${notes ? NOTE("01 / Accent has a job.", `The main action uses <code>bg-accent</code> (${v.accent}) and <code>text-on-accent</code> (${onAccent(v)}). Ordinary text stays neutral, so emphasis remains useful.`) : ""}<section class="section" id="journal" data-inspect="space"><div class="section-title"><h2>Recent notes</h2><span>Two things worth keeping</span></div><div class="cards"><article class="ds-card"><div class="card-meta"><span>DESIGN</span><span>04 OCT 2026</span></div><h3>余白に、役割を持たせる。</h3><p>Space can show what belongs together. A few named decisions make a page easier to read.</p><div class="card-foot"><span>4 min read</span><button type="button" class="bookmark" data-demo="bookmark" aria-pressed="false">Save note</button></div></article><article class="ds-card"><div class="card-meta"><span>PROCESS</span><span>01 OCT 2026</span></div><h3>A smaller vocabulary.</h3><p>Give the repeated decisions a name. Keep the unusual details close to where you use them.</p><div class="card-foot"><span>3 min read</span><button type="button" class="bookmark" data-demo="bookmark" aria-pressed="false">Save note</button></div></article></div></section>${notes ? NOTE("02 / A gap describes a relationship.", `Vertical groups use <code>gap-y-vsp-stack</code> (${v.groupGap}px); columns use <code>gap-x-hsp-gutter</code> (${v.horizontalGap}px). Separate sections use <code>mt-vsp-section</code> (${v.sectionGap}px). Card padding uses the independent horizontal token <code>px-hsp-card</code> (${v.horizontalSpace}px).`) : ""}<section class="section"><form class="subscribe" id="demo-form" method="dialog"><div><h3>A note, now and then.</h3><p>A small signup form shows the same rules at work.</p></div><div class="form-fields"><label for="sample-email">Email address</label><div class="input-row"><input id="sample-email" class="ds-input" type="email" required placeholder="you@example.com" autocomplete="off"><button class="ds-button" data-inspect="accent" type="submit">Try signup</button></div><span class="form-note" id="signup-status" role="status">Demo only. Nothing is sent or stored.</span></div></form></section><article class="section reading-sample" id="how-it-works"><p class="reading-note-title">HOW THIS SITE IS STYLED</p><h2>Small rules, used together.</h2><p data-inspect="body">This starter has a few named colors, separate horizontal and vertical spacing, and type roles for headings and body text. You can change those decisions in your project.</p><p>たくさんの値を覚えるより、何のための値なのかを知ること。<code>text-body</code> は読みやすさを、<code>vsp-section</code> はまとまりの違いを表します。</p><p>The utility engine turns your vocabulary into CSS. The design comes from the choices you make here—and the way you keep using them.</p></article>${notes ? NOTE("03 / Keep the reason beside the value.", `Body text uses ${v.bodySize}px with ${v.lineHeight} line height and a ${v.readingWidth}ch reading measure. Headings use ${v.headingFont}. Cards, fields and actions share ${v.radius === 0 ? "square corners" : v.radius + "px corners"}. These examples and their short usage rules travel with your files.`) : ""}<footer class="sample-footer"><span>Fieldnotes / a ZFB starter example</span><span>Built from your own decisions.</span></footer></main>`;
}
function componentMarkup() {
  return `<main class="page-wrap"><header class="specimen-header"><p class="kicker">Component examples</p><h1>One vocabulary, several places.</h1><p>These examples share the same roles as the page.</p></header><section class="specimen"><p class="spec-label">Actions / primary · secondary · disabled</p><div class="button-sample"><button class="ds-button" data-inspect="accent" data-demo="sample-action">Primary action</button><button class="ds-button secondary" data-demo="sample-action">Secondary</button><button class="ds-button secondary" disabled>Unavailable</button></div></section><section class="specimen" data-inspect="space"><p class="spec-label">Card / grouping and horizontal padding</p><article class="ds-card"><div class="card-meta">DESIGN NOTE <span>4 MIN READ</span></div><h3>余白は、情報の関係をつくる。</h3><p data-inspect="body">A card uses the same body role, corner rule and spacing vocabulary as the rest of the site.</p><div class="card-foot"><span>A reusable example</span><button class="bookmark" data-demo="bookmark" aria-pressed="false">Save note</button></div></article></section><section class="specimen"><p class="spec-label">Field / label · input · helper</p><div class="field-example"><label for="specimen-name">Project name</label><input class="ds-input" id="specimen-name" placeholder="My fieldnotes" value="My fieldnotes"><span class="form-note">Use a name that is easy to recognize.</span></div></section><section class="specimen"><p class="spec-label">Notice / quiet and useful</p><div class="message"><b>Your work has a home.</b><p>Keep notes with the project so the next person can understand the choices.</p></div></section><section class="specimen"><p class="spec-label">State / text carries the meaning</p><div class="button-sample"><span class="state-label">Draft</span><span class="state-label">Ready for review</span></div></section></main>`;
}
function foundationMarkup(state) {
  const v = state.values;
  validateValues(v);
  return `<main class="page-wrap"><header class="specimen-header"><p class="kicker">Design foundations</p><h1>A small vocabulary.</h1><p>Start with the decisions you repeat. Add others when the design needs them.</p></header><section class="specimen"><p class="spec-label">Color / semantic roles</p><div class="token-colors">${["background", "surface", "ink", "muted", "accent"].map((k) => `<div class="color-sample"><div class="swatch" style="background:var(--ds-${k})"></div><b>${k}</b><code data-value-color="${k}">${v[k]}</code></div>`).join("")}</div></section><section class="specimen"><p class="spec-label">Typography / a role is more than a size</p><div class="type-sample"><label>heading</label><h2>Room to grow.</h2></div><div class="type-sample" data-inspect="body"><label>body</label><p>小さな発見を、言葉にする。<br>A comfortable rhythm for reading.</p></div><div class="type-sample"><label>caption</label><small>04 OCT 2026 · Supporting context</small></div></section><section class="specimen" data-inspect="space"><p class="spec-label">Spacing / name the job</p><div class="spacing-sample"><div class="spacing-row"><code>hsp-card</code><div class="space-bar" style="width:var(--ds-hsp-card)"></div></div><div class="spacing-row"><code>vsp-stack</code><div class="space-bar" style="width:var(--ds-vsp-stack)"></div></div><div class="spacing-row"><code>vsp-section</code><div class="space-bar" style="width:var(--ds-vsp-section)"></div></div></div><div class="group-example" aria-label="Two groups of content separated by section spacing"><div class="example-group"><div></div><div></div><div></div></div><div class="example-group"><div></div><div></div><div></div></div></div></section><section class="specimen"><p class="spec-label">Shape / one declared default</p><div class="ds-card"><h3>Same radius, shared rule.</h3><p>A new shape should be a deliberate exception.</p></div></section></main>`;
}
function specimenScript(withInspection) {
  return `
(()=>{let timer;function toast(message){const e=document.getElementById('sample-toast');e.textContent=message;e.hidden=false;clearTimeout(timer);timer=setTimeout(()=>e.hidden=true,2500)}
document.addEventListener('click',event=>{${withInspection ? `const inspect=event.target.closest('[data-inspect]');if(document.body.classList.contains('inspect-on')&&inspect){event.preventDefault();event.stopPropagation();parent.postMessage({type:'zfb-workshop-inspect',role:inspect.dataset.inspect},'*');return;}` : ""}const anchor=event.target.closest('a[href^="#"]');if(anchor){const target=document.getElementById(anchor.getAttribute('href').slice(1));if(target){event.preventDefault();target.scrollIntoView({behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'instant':'smooth',block:'start'});return;}}const el=event.target.closest('[data-demo]');if(!el)return;if(el.dataset.demo==='bookmark'){const yes=el.getAttribute('aria-pressed')!=='true';el.setAttribute('aria-pressed',String(yes));el.textContent=yes?'Saved':'Save note';}if(el.dataset.demo==='sample-action')toast('Example action selected.');if(el.dataset.demo==='about'){const panel=document.getElementById('about-panel');panel.hidden=!panel.hidden;el.setAttribute('aria-expanded',String(!panel.hidden));}});
document.addEventListener('submit',event=>{if(event.target.id!=='demo-form')return;event.preventDefault();const status=document.getElementById('signup-status');status.textContent='Preview complete. No signup was submitted.';toast('Demo only — no information was sent.');});
${withInspection ? `document.addEventListener('keydown',event=>{if(event.key==='Enter'&&document.body.classList.contains('inspect-on')&&event.target.hasAttribute('data-inspect')){event.preventDefault();parent.postMessage({type:'zfb-workshop-inspect',role:event.target.dataset.inspect},'*');}});` : ""}
})();`;
}
function previewDocument(
  state,
  mode = "page",
  notes = false,
  standalone = false,
  inspection = false,
) {
  state = restoreSnapshot(state);
  const title =
    mode === "components"
      ? "Component examples"
      : mode === "foundations"
        ? "Design foundations"
        : "Fieldnotes";
  let markup =
    mode === "components"
      ? componentMarkup()
      : mode === "foundations"
        ? foundationMarkup(state)
        : pageMarkup(state, notes);
  const css = standalone ? PREVIEW_CSS.replace(/\.inspect-on[^}]*}/g, "") : PREVIEW_CSS;
  const script = specimenScript(!standalone);
  if (standalone) markup = markup.replace(/ data-inspect="[^"]*"/g, "");
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title} · ${PRESETS[state.preset].name}</title><style id="ds-variables">${variablesCSS(state.values)}</style><style>${css}</style></head><body${inspection && !standalone ? ' class="inspect-on"' : ""}>${markup}<div class="sample-toast" id="sample-toast" role="status" hidden></div><script>${script}<\/script></body></html>`;
}
function windObject() {
  return {
    spec: 1,
    reset: "owned-v1",
    tokens: {
      colors: {
        background: "var(--ds-background)",
        surface: "var(--ds-surface)",
        ink: "var(--ds-ink)",
        muted: "var(--ds-muted)",
        border: "var(--ds-border)",
        accent: "var(--ds-accent)",
        "on-accent": "var(--ds-on-accent)",
      },
      spacing: {
        "hsp-page": "var(--ds-hsp-page)",
        "hsp-card": "var(--ds-hsp-card)",
        "hsp-gutter": "var(--ds-hsp-gutter)",
        "vsp-stack": "var(--ds-vsp-stack)",
        "vsp-section": "var(--ds-vsp-section)",
      },
      sizes: { reading: "var(--ds-size-reading)" },
      fontSizes: {
        body: { size: "var(--ds-font-body)", lineHeight: "var(--ds-leading-body)" },
        heading: { size: "var(--ds-font-heading)", lineHeight: "1.22" },
        section: { size: "var(--ds-font-section)", lineHeight: "1.3" },
        caption: { size: "var(--ds-font-caption)", lineHeight: "1.5" },
      },
      fontFamilies: { ui: "var(--ds-font-ui)", display: "var(--ds-font-display)" },
      fontWeights: { regular: "400", strong: "600" },
      radii: { panel: "var(--ds-radius-panel)" },
    },
    breakpoints: { wide: { minWidthPx: 960 } },
  };
}
function getSeedFiles(state) {
  state = restoreSnapshot(state);
  const v = state.values,
    p = PRESETS[state.preset];
  const config = `// ${p.name} design seed — editable project-owned choices.\n// Merge this wind section with your existing ZFB configuration.\n// Targets the current ZFB Wind spec 1 configuration.\n// demo.html is an authored-CSS specimen, not compiler output.\nexport const wind = ${JSON.stringify(windObject(v), null, 2)} as const;\n`;
  const css = `/* ${p.name} — values, roles and usage travel together.\n   Edit these variables in your project's stylesheet.\n   Do not author values in the engine-reserved --zw-* namespace.\n*/\n${variablesCSS(v)}`;
  const usage = `// Complete utility-class literals, using this seed's vocabulary.\n// Load design-system.css through your existing stylesheet entry.\n// Your ZFB build compiles these utility classes when the component is used.\n// The demo.html file is\n// a separate authored-CSS visual specimen and does not run the compiler.\nexport function DesignExample() {\n  return (\n    <section class="font-ui bg-surface text-ink px-hsp-card py-vsp-stack rounded-panel">\n      <h2 class="font-display text-section font-strong">Small ideas.</h2>\n      <p class="text-body max-w-reading mt-vsp-stack">\n        Give the repeated decisions a name.\n      </p>\n      <div class="flex flex-wrap gap-x-hsp-gutter gap-y-vsp-stack mt-vsp-section">\n        <button class="bg-accent text-on-accent px-hsp-card py-vsp-stack rounded-panel">\n          Primary action\n        </button>\n      </div>\n    </section>\n  );\n}\n`;
  const readme = `# ${p.name} design seed\n\nGenerated from the zudo-wind Design system playground.\nThis is a curated example you own, not an engine default.\n\n## Use these files\n\n1. Open demo.html directly to inspect the design without installing anything.\n2. Copy design-system.css into your project and load it through the existing stylesheet entry.\n3. Import the wind export from zfb.design.ts into your existing zfb.config.ts and merge deliberately with the current wind section. Keep unrelated configuration, routes, plugins and stylesheet paths.\n4. Merge token maps by category and then by token name. Do not replace the entire existing tokens object while existing pages still depend on its values.\n5. Copy usage.tsx into your existing component source tree and render it from a page. Run your project's normal ZFB build to compile utilities and validate integration.\n\nThe live demo uses authored CSS consuming the same CSS variables. It is a visual specimen, not the result of running zudo-wind. The Wind section targets ZFB Wind spec 1. Build the utility example in your project to validate its integration.\n\n## Design decisions\n\n${currentRules(
    state,
  )
    .map((r) => "- " + r)
    .join(
      "\n",
    )}\n\n## What the names mean\n\n- hsp-card and hsp-page: horizontal padding decisions.\n- hsp-gutter: independently named horizontal column gaps.\n- vsp-stack: vertical space inside a group.\n- vsp-section: space between separate groups.\n- body, heading, section and caption: font-size roles with explicit line height.\n- accent and on-accent: primary-action background and its readable label.\n- reading: a bounded text measure.\n- panel: the declared corner rule used by cards, inputs and buttons.\n\nA small vocabulary makes repeated decisions visible. Structural one-offs can remain close to their component. Add a new token when it represents a real reusable decision.\n\n## Scope and ownership\n\nThese files are standalone project inputs. No dependency on the workshop, styleguide engine or token panel is required. This is not a complete ZFB app: it has no routing, content collection or build scripts. Existing ZFB template selection is separate. No --preset or --design-seed CLI option is claimed here.\n\nThe seed includes no light/dark switch; Workbench is a distinct dark seed. Add mode mappings deliberately if your project needs both. The button label color is selected for contrast against the accent, not a claim of whole-page accessibility certification.\n\n## Research sources\n\n- https://github.com/Takazudo/zudo-front-builder\n- https://github.com/Takazudo/zudo-css-wisdom\n- https://github.com/Takazudo/zudo-sg\n- https://github.com/Takazudo/zudo-design-token-panel\n`;
  return {
    "zfb.design.ts": config,
    "design-system.css": css,
    "usage.tsx": usage,
    "DESIGN-NOTES.md": readme,
    "demo.html": previewDocument(state, "page", true, true),
  };
}
function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (c & 1 ? 0xedb88320 : 0);
  }
  return (c ^ 0xffffffff) >>> 0;
}
function zipData(files) {
  const encoder = new TextEncoder(),
    parts = [],
    central = [];
  let offset = 0;
  const put16 = (b, o, v) => new DataView(b.buffer).setUint16(o, v, true),
    put32 = (b, o, v) => new DataView(b.buffer).setUint32(o, v, true);
  for (const [name, content] of Object.entries(files)) {
    const filename = encoder.encode(name),
      data = encoder.encode(content),
      crc = crc32(data),
      local = new Uint8Array(30 + filename.length);
    put32(local, 0, 0x04034b50);
    put16(local, 4, 20);
    put16(local, 6, 0x0800);
    put16(local, 8, 0);
    put16(local, 12, 33);
    put32(local, 14, crc);
    put32(local, 18, data.length);
    put32(local, 22, data.length);
    put16(local, 26, filename.length);
    local.set(filename, 30);
    parts.push(local, data);
    const c = new Uint8Array(46 + filename.length);
    put32(c, 0, 0x02014b50);
    put16(c, 4, 20);
    put16(c, 6, 20);
    put16(c, 8, 0x0800);
    put16(c, 14, 33);
    put32(c, 16, crc);
    put32(c, 20, data.length);
    put32(c, 24, data.length);
    put16(c, 28, filename.length);
    put32(c, 42, offset);
    c.set(filename, 46);
    central.push(c);
    offset += local.length + data.length;
  }
  const centralSize = central.reduce((n, b) => n + b.length, 0),
    end = new Uint8Array(22);
  put32(end, 0, 0x06054b50);
  put16(end, 8, central.length);
  put16(end, 10, central.length);
  put32(end, 12, centralSize);
  put32(end, 16, offset);
  const chunks = [...parts, ...central, end],
    output = new Uint8Array(offset + centralSize + end.length);
  let cursor = 0;
  for (const chunk of chunks) {
    output.set(chunk, cursor);
    cursor += chunk.length;
  }
  return output;
}

function deepFreeze(value) {
  for (const child of Object.values(value)) {
    if (child && typeof child === "object") deepFreeze(child);
  }
  return Object.freeze(value);
}
deepFreeze(PRESETS);
deepFreeze(FONTS);
deepFreeze(KEYS);
deepFreeze(LIMITS);

export const SNAPSHOT_VERSION = 1;
const hasOwn = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
const isRecord = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

/** Guard the low-level values boundary for Node and browser callers. */
function validateValues(values) {
  if (!isRecord(values)) throw new TypeError("Invalid design values");
  for (const key of KEYS) {
    if (!isValidValue(key, values[key])) throw new TypeError("Invalid design value: " + key);
  }
  for (const key of ["background", "surface", "ink", "muted", "border"]) {
    if (typeof values[key] !== "string" || !HEX.test(values[key])) {
      throw new TypeError("Invalid design color: " + key);
    }
  }
  for (const key of ["horizontalGap", "headingSize"]) {
    if (
      typeof values[key] !== "number" ||
      !Number.isFinite(values[key]) ||
      values[key] <= 0 ||
      values[key] > 100
    ) {
      throw new TypeError("Invalid design value: " + key);
    }
  }
}

/** Create an independent mutable browser-session state. */
export function createState(preset = "everyday") {
  if (typeof preset !== "string" || !hasOwn(PRESETS, preset)) preset = "everyday";
  return {
    preset,
    values: { ...PRESETS[preset].values },
    preview: "page",
    width: "full",
    inspect: false,
    role: "accent",
    notes: true,
    route: "start",
  };
}

/** Validate editable decisions; preset-owned foundations cannot be overwritten. */
export function isValidValue(key, value) {
  if (!KEYS.includes(key)) return false;
  if (key === "accent") return typeof value === "string" && HEX.test(value);
  if (key === "headingFont") return typeof value === "string" && hasOwn(FONTS, value);
  const [min, max] = LIMITS[key];
  return typeof value === "number" && Number.isFinite(value) && value >= min && value <= max;
}

/** Mutates only the caller's state and never invalidates the previous valid value. */
export function setValue(state, key, value) {
  if (!isValidValue(key, value)) return false;
  state.values[key] = key === "accent" ? value.toLowerCase() : value;
  return true;
}

/**
 * Restore only known fields from a snapshot, without trusting arbitrary CSS.
 * Versionless round-2 snapshots are supported; unknown versions reset safely.
 * Finite numeric controls clamp to the accepted range; invalid values fall back.
 * Backgrounds, gutters and heading size always come from the selected seed.
 */
export function restoreSnapshot(saved) {
  if (!isRecord(saved) || (hasOwn(saved, "version") && saved.version !== SNAPSHOT_VERSION)) {
    return createState();
  }
  const state = createState(hasOwn(saved, "preset") ? saved.preset : undefined);
  if (hasOwn(saved, "values") && isRecord(saved.values)) {
    for (const key of KEYS) {
      if (!hasOwn(saved.values, key)) continue;
      const value = saved.values[key];
      if (hasOwn(LIMITS, key) && typeof value === "number" && Number.isFinite(value)) {
        const [min, max] = LIMITS[key];
        setValue(state, key, Math.max(min, Math.min(max, value)));
      } else {
        setValue(state, key, value);
      }
    }
  }
  for (const [key, choices] of Object.entries({
    preview: ["page", "components", "foundations"],
    width: ["full", "mobile"],
    role: ["accent", "body", "space"],
    route: ["start", "playground", "starter"],
  })) {
    if (hasOwn(saved, key) && choices.includes(saved[key])) state[key] = saved[key];
  }
  for (const key of ["inspect", "notes"]) {
    if (hasOwn(saved, key) && typeof saved[key] === "boolean") state[key] = saved[key];
  }
  return state;
}

/** Detach and validate the retained snapshot; no storage or network side effects. */
export function createSnapshot(state) {
  return { version: SNAPSHOT_VERSION, ...restoreSnapshot(state) };
}

export function zipBytes(files) {
  return new Blob([zipData(files)], { type: "application/zip" });
}

export {
  PRESETS,
  FONTS,
  KEYS,
  HEX,
  LIMITS,
  luminance,
  contrast,
  onAccent,
  variableMap,
  variablesCSS,
  currentRules,
  pageMarkup,
  componentMarkup,
  foundationMarkup,
  previewDocument,
  windObject,
  getSeedFiles,
  crc32,
  zipData,
};
