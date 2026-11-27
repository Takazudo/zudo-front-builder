// Shared by the persist-focus-*.html pages. The same src runs once per real
// document, so the listener below survives every SPA swap.
import { init, navigate } from "/dist/client-router/router.js";

init();
window.__pfNavigate = (href) => navigate(href);
document.addEventListener("keydown", (event) => {
  const href = window.__pfEnterTarget;
  if (event.key !== "Enter" || !href || !event.target.closest?.("[data-zfb-transition-persist]"))
    return;
  event.preventDefault();
  window.__pfEnterTarget = undefined;
  navigate(href);
});
