/** Native docs mount; specimen and export generation stay in the shared pure model. */
import {
  PRESETS,
  FONTS,
  KEYS,
  HEX,
  contrast,
  onAccent,
  variablesCSS,
  restoreSnapshot,
  createSnapshot,
  setValue as updateValue,
  currentRules as rules,
  previewDocument as specimen,
  getSeedFiles as seedFiles,
  zipBytes,
} from "./model.js";
import { WORKSHOP_MARKUP } from "./markup.js";
export function mountDesignWorkshop(root, options = {}) {
  const state = restoreSnapshot(options.initialState);
  const getSeedFiles = () => seedFiles(state);
  const currentRules = () => rules(state);
  const previewDocument = (...args) => specimen(state, ...args);
  let exportFiles = {},
    currentExport = "zfb.design.ts",
    toastTimer,
    activeExportZipUrl;
  const $ = (s) => root.querySelector(s);
  const $$ = (s) => Array.from(root.querySelectorAll(s));
  const esc = (v) =>
    String(v).replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
    );
  function renderPresets() {
    const grid = $("#preset-grid");
    grid.innerHTML = Object.entries(PRESETS)
      .map(([id, p]) => {
        const v = p.values;
        return `<button type="button" class="preset-card" data-preset="${id}" aria-pressed="${id === state.preset}" aria-label="Choose ${p.name}: ${p.description}"><div class="mini-preview" style="background:${v.background};color:${v.ink};font-family:${esc(FONTS[v.headingFont])}"><div class="mini-nav"><span>FIELDNOTES</span><span>Journal &nbsp; About</span></div><p class="mini-kicker">Notes on making things</p><div class="mini-title">Small ideas.<br>Room to grow.</div><p class="mini-copy">つくること、考えること。<br>A journal of useful details and experiments.</p><div class="mini-bottom"><span class="mini-cta" style="background:${v.accent};color:${onAccent(v)};border-radius:${Math.min(v.radius, 6)}px">Read the journal</span><span class="mini-line"></span><span class="mini-tag">01 / 03</span></div></div><div class="preset-info"><div class="preset-info-top"><h3>${p.name}</h3><span class="preset-check" aria-hidden="true">✓</span></div><span class="preset-desc">${p.description}</span><div class="preset-traits">${id === "everyday" ? '<span class="suggested">Suggested start</span>' : ""}${p.traits.map((t) => `<span>${t}</span>`).join("")}</div></div></button>`;
      })
      .join("");
  }
  function choosePreset(id, { notify = false } = {}) {
    if (!PRESETS[id]) return false;
    state.preset = id;
    state.values = { ...PRESETS[id].values };
    renderPresets();
    renderControls();
    renderPreview();
    renderStarter();
    renderSelection();
    renderInspector();
    if (notify) toast(PRESETS[id].name + " loaded as your starting point.");
    return true;
  }
  function renderSelection() {
    const p = PRESETS[state.preset];
    $("#chosen-name").textContent = p.name;
    $("#chosen-description").textContent = p.description;
    $("#editor-preset").value = state.preset;
    $("#frame-address-label").textContent = "your-site / " + p.name;
    $("#starter-address").textContent = "your-site / " + p.name;
    $("#starter-preset-name").textContent = p.name;
    $("#starter-preset-desc").textContent = p.detail;
    $("#starter-rules").innerHTML = currentRules()
      .map((x) => "<li>" + esc(x) + "</li>")
      .join("");
    const changes = KEYS.filter((k) => state.values[k] !== p.values[k]).length;
    $("#change-count").textContent = changes
      ? changes + " changed " + (changes === 1 ? "decision" : "decisions")
      : "Original preset";
  }
  function renderControls() {
    const v = state.values;
    $("#accent-color").value = v.accent;
    $("#accent-hex").value = v.accent;
    $("#color-error").hidden = true;
    $("#accent-hex").removeAttribute("aria-invalid");
    $("#heading-font").value = v.headingFont;
    const controls = {
      "body-size": "bodySize",
      "line-height": "lineHeight",
      "group-gap": "groupGap",
      "section-gap": "sectionGap",
      "horizontal-space": "horizontalSpace",
      radius: "radius",
      "reading-width": "readingWidth",
    };
    for (const [id, key] of Object.entries(controls)) {
      $("#" + id).value = v[key];
      $("#" + id + "-out").value =
        v[key] + (key === "lineHeight" ? "" : key === "readingWidth" ? " ch" : " px");
    }
    renderColorInfo();
    renderRhythm();
    renderSelection();
  }
  function renderColorInfo() {
    const v = state.values;
    $("#role-swatches").innerHTML = ["surface", "ink", "muted"]
      .map((k) => `<span class="role-swatch"><i style="background:${v[k]}"></i>${k}</span>`)
      .join("");
    $("#contrast-line").innerHTML =
      `<span class="status-shape" aria-hidden="true">✓</span> Button label contrast ${contrast(v.accent, onAccent(v)).toFixed(1)}:1`;
  }
  function renderRhythm() {
    const v = state.values,
      ratio = v.sectionGap / v.groupGap;
    $("#rhythm-readout").innerHTML =
      `<span class="rhythm-bars" aria-hidden="true"><i style="width:${(v.groupGap / 64) * 48}px"></i><i style="width:${(v.sectionGap / 64) * 48}px"></i></span><span>${ratio.toFixed(1)}× between groups.<br>${ratio >= 2 ? "More room marks a new section." : "Try a larger gap to separate groups."}</span>`;
  }
  function renderPreview() {
    const frame = $("#design-preview");
    frame.srcdoc = previewDocument(state.preview, false, false, state.inspect);
    $("#frame-shell").classList.toggle("phone", state.width === "mobile");
    $("#frame-size").textContent = state.width === "mobile" ? "390 px max" : "Live preview";
    $$("[data-preview]").forEach((el) => {
      el.classList.toggle("selected", el.dataset.preview === state.preview);
      el.setAttribute("aria-pressed", String(el.dataset.preview === state.preview));
    });
    $$("[data-width]").forEach((el) => {
      el.classList.toggle("selected", el.dataset.width === state.width);
      el.setAttribute("aria-pressed", String(el.dataset.width === state.width));
    });
  }
  function renderStarter() {
    $("#starter-preview").srcdoc = previewDocument("page", state.notes);
  }
  function applyVariables() {
    for (const id of ["design-preview", "starter-preview"]) {
      const doc = $("#" + id).contentDocument;
      if (!doc) continue;
      const style = doc.getElementById("ds-variables");
      if (style) style.textContent = variablesCSS(state.values);
      doc
        .querySelectorAll("[data-value-color]")
        .forEach((e) => (e.textContent = state.values[e.dataset.valueColor]));
    }
    renderColorInfo();
    renderRhythm();
    renderSelection();
    renderInspector();
  }
  function setValue(key, value) {
    if (!updateValue(state, key, value)) return false;
    applyVariables();
    renderStarter();
    if (state.preview === "foundations") renderPreview();
    return true;
  }
  function renderInspector() {
    const v = state.values;
    let parts, explanation;
    if (state.role === "body") {
      parts = [
        ["Value", v.bodySize + "px / " + v.lineHeight, "Font size and line height"],
        ["Role", "--ds-font-body", "The text people read"],
        ["Usage", "text-body", "Intro, paragraphs, fields"],
      ];
      explanation =
        "Size is the value. Body is the role. Reusing that role keeps the reading rhythm consistent.";
    } else if (state.role === "space") {
      parts = [
        ["Value", v.groupGap + "px / " + v.sectionGap + "px", "Within / between groups"],
        ["Role", "vsp-stack / vsp-section", "Separate named relationships"],
        ["Usage", "gap-y-vsp-stack", "Cards and grouped content"],
      ];
      explanation =
        "Change the relationship between gaps, then judge the page. Using tokens alone does not guarantee clear grouping.";
    } else {
      parts = [
        ["Value", v.accent, "--ds-brand"],
        ["Role", "--ds-accent", "var(--ds-brand)"],
        ["Usage", "bg-accent", "Primary actions and selections"],
      ];
      explanation =
        "The color value feeds an accent role. Components use that role, so one change reaches each intended place.";
    }
    $("#token-chain").innerHTML = parts
      .map(
        (p) =>
          `<div class="chain-step"><span class="chain-label">${p[0]}</span><code>${esc(p[1])}</code><small>${esc(p[2])}</small></div>`,
      )
      .join("");
    $("#role-explanation").textContent = explanation;
    $$("[data-role]").forEach((e) => {
      e.classList.toggle("selected", e.dataset.role === state.role);
      e.setAttribute("aria-pressed", String(e.dataset.role === state.role));
    });
    $("#inspect-button").setAttribute("aria-pressed", String(state.inspect));
    $("#inspect-helper").textContent = state.inspect
      ? "Select an outlined part of the preview."
      : "Pick a role, or turn on Inspect roles.";
  }
  function setInspection(enabled) {
    state.inspect = enabled;
    const doc = $("#design-preview").contentDocument;
    if (doc?.body) {
      doc.body.classList.toggle("inspect-on", enabled);
      doc.querySelectorAll("[data-inspect]").forEach((el) => {
        if (enabled) el.setAttribute("tabindex", "0");
        else if (!["A", "BUTTON", "INPUT"].includes(el.tagName)) el.removeAttribute("tabindex");
      });
    }
    renderInspector();
  }
  function navigate(next = "start", scroll = true) {
    const route = ["start", "playground", "starter"].includes(next) ? next : "start";
    state.route = route;
    for (const id of ["start", "playground", "starter"])
      $("#" + id + "-view").hidden = id !== route;
    $$("[data-route]").forEach((e) => {
      if (e.dataset.route === route) e.setAttribute("aria-current", "page");
      else e.removeAttribute("aria-current");
    });
    if (route === "playground") renderPreview();
    if (route === "starter") renderStarter();
    root.dataset.view = route;
    if (scroll) root.scrollIntoView({ block: "start", behavior: "instant" });
  }
  function renderExport() {
    exportFiles = getSeedFiles();
    const names = Object.keys(exportFiles);
    if (!names.includes(currentExport)) currentExport = names[0];
    $("#export-file-tabs").innerHTML = names
      .map(
        (f) =>
          `<button class="${f === currentExport ? "selected" : ""}" data-export-file="${f}" aria-pressed="${f === currentExport}">${f}</button>`,
      )
      .join("");
    $("#export-file-name").textContent = currentExport;
    $("#export-code").textContent = exportFiles[currentExport];
  }
  function openExport() {
    renderExport();
    if (activeExportZipUrl) URL.revokeObjectURL(activeExportZipUrl);
    activeExportZipUrl = URL.createObjectURL(zipBytes(exportFiles));
    const link = $("#download-seed");
    link.href = activeExportZipUrl;
    link.download = "zudo-wind-" + state.preset + "-seed.zip";
    $("#export-dialog").showModal();
  }
  function renderInitializer() {
    const template = $("#init-template").value,
      seed = $("#init-seed").value;
    $("#initializer-summary").innerHTML =
      seed === "none"
        ? `<b>${template === "basic-blog" ? "Basic blog" : "Node-free"} + your own design</b><br>Keep the content structure and author your own wind tokens. No curated design files would be added.`
        : `<b>${template === "basic-blog" ? "Basic blog" : "Node-free"} + ${PRESETS[seed].name}</b><br>Project structure + editable CSS, a wind configuration, sample usage and design notes.`;
    $('#initializer-dialog [data-action="init-continue"]').textContent =
      seed === "none" ? "Return to designs" : "Explore this seed";
  }
  async function copyFile() {
    const filename = currentExport,
      str = exportFiles[filename];
    try {
      await navigator.clipboard.writeText(str);
      if (lifecycle.signal.aborted) return;
      toast(filename + " copied.");
    } catch {
      if (lifecycle.signal.aborted) return;
      const area = document.createElement("textarea");
      area.value = str;
      area.style.position = "fixed";
      area.style.opacity = "0";
      $("#export-dialog").appendChild(area);
      area.select();
      let success = false;
      try {
        success = document.execCommand("copy");
      } catch {}
      area.remove();
      toast(success ? filename + " copied." : "Select the code and copy it with your browser.");
    }
  }
  function toast(message) {
    const el = $("#toast");
    el.textContent = message;
    el.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      el.hidden = true;
    }, 3500);
  }
  root.innerHTML = WORKSHOP_MARKUP;
  const hadWorkshopClass = root.classList.contains("zwd-workshop");
  root.classList.add("zwd-workshop");
  const lifecycle = new AbortController();
  const listen = (target, type, handler) =>
    target.addEventListener(type, handler, { signal: lifecycle.signal });
  listen(root, "click", (event) => {
    const el = event.target.closest("button,a");
    if (!el || !root.contains(el)) return;
    if (el.dataset.view) {
      event.preventDefault();
      navigate(el.dataset.view);
      return;
    }
    if (el.dataset.preset) {
      choosePreset(el.dataset.preset);
      return;
    }
    if (el.dataset.preview) {
      state.preview = el.dataset.preview;
      renderPreview();
      return;
    }
    if (el.dataset.width) {
      state.width = el.dataset.width;
      renderPreview();
      return;
    }
    if (el.dataset.role) {
      state.role = el.dataset.role;
      renderInspector();
      return;
    }
    if (el.dataset.exportFile) {
      currentExport = el.dataset.exportFile;
      renderExport();
      return;
    }
    if (el.hasAttribute("data-close")) {
      el.closest("dialog").close();
      return;
    }
    switch (el.dataset.action) {
      case "reset":
        choosePreset(state.preset);
        toast("Reset to the " + PRESETS[state.preset].name + " starting point.");
        break;
      case "inspect":
        setInspection(!state.inspect);
        break;
      case "export":
        openExport();
        break;
      case "copy-file":
        copyFile();
        break;
      case "open-initializer":
        $("#init-seed").value = state.preset;
        renderInitializer();
        $("#initializer-dialog").showModal();
        break;
      case "init-continue": {
        const id = $("#init-seed").value;
        $("#initializer-dialog").close();
        if (id !== "none") {
          choosePreset(id);
          navigate("playground");
        } else {
          navigate("start");
          toast("The proposed initializer keeps a path for your own system.");
        }
        break;
      }
      case "sources":
        $("#sources-dialog").showModal();
        break;
    }
  });
  listen($("#editor-preset"), "change", (e) => choosePreset(e.target.value, { notify: true }));
  listen($("#accent-color"), "input", (e) => {
    if (setValue("accent", e.target.value)) {
      $("#accent-hex").value = e.target.value;
      $("#color-error").hidden = true;
      $("#accent-hex").removeAttribute("aria-invalid");
    }
  });
  listen($("#accent-hex"), "input", (e) => {
    const good = HEX.test(e.target.value);
    $("#color-error").hidden = good;
    e.target.setAttribute("aria-invalid", String(!good));
    if (good) {
      setValue("accent", e.target.value);
      $("#accent-color").value = e.target.value;
    }
  });
  listen($("#accent-hex"), "blur", (e) => {
    if (!HEX.test(e.target.value)) {
      e.target.value = state.values.accent;
      e.target.removeAttribute("aria-invalid");
      $("#color-error").hidden = true;
    }
  });
  listen($("#heading-font"), "change", (e) => setValue("headingFont", e.target.value));
  for (const [id, key] of Object.entries({
    "body-size": "bodySize",
    "line-height": "lineHeight",
    "group-gap": "groupGap",
    "section-gap": "sectionGap",
    "horizontal-space": "horizontalSpace",
    radius: "radius",
    "reading-width": "readingWidth",
  }))
    listen($("#" + id), "input", (e) => {
      const v = Number(e.target.value);
      if (setValue(key, v))
        $("#" + id + "-out").value =
          v + (key === "lineHeight" ? "" : key === "readingWidth" ? " ch" : " px");
    });
  listen($("#show-notes"), "change", (e) => {
    state.notes = e.target.checked;
    renderStarter();
  });
  listen($("#init-template"), "change", renderInitializer);
  listen($("#init-seed"), "change", renderInitializer);
  listen($("#design-preview"), "load", () => {
    applyVariables();
    setInspection(state.inspect);
  });
  listen($("#starter-preview"), "load", applyVariables);
  listen(window, "message", (e) => {
    if (
      e.source !== $("#design-preview").contentWindow ||
      e.data?.type !== "zfb-workshop-inspect" ||
      !["accent", "body", "space"].includes(e.data.role)
    )
      return;
    state.role = e.data.role;
    renderInspector();
    if (window.innerWidth < 600)
      $("#role-inspector").scrollIntoView({
        behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth",
        block: "nearest",
      });
  });
  for (const dialog of $$("dialog"))
    listen(dialog, "click", (e) => {
      if (e.target !== dialog) return;
      const r = dialog.getBoundingClientRect();
      if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)
        dialog.close();
    });
  renderPresets();
  renderControls();
  renderInspector();
  renderSelection();
  $("#show-notes").checked = state.notes;
  navigate(state.route, false);
  let disposed = false;
  return {
    getSnapshot: () => createSnapshot(state),
    getSeedFiles,
    zipBytes,
    PRESETS,
    navigate,
    dispose() {
      if (disposed) return;
      disposed = true;
      lifecycle.abort();
      clearTimeout(toastTimer);
      if (activeExportZipUrl) URL.revokeObjectURL(activeExportZipUrl);
      for (const dialog of $$("dialog[open]")) dialog.close();
      root.replaceChildren();
      delete root.dataset.view;
      if (!hadWorkshopClass) root.classList.remove("zwd-workshop");
    },
  };
}
