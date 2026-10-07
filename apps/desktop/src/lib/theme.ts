/// Lathe has one fixed appearance, shared with the pre-paint markup in index.html.
export const APP_THEME = "one-dark-pro";
export const APP_MODE = "dark";

/// Keep shadcn/Streamdown dark variants and native webview controls in sync.
export function applyTheme() {
  const el = document.documentElement;
  el.dataset.theme = APP_THEME;
  el.dataset.mode = APP_MODE;
  el.classList.add("dark");
  el.style.colorScheme = APP_MODE;
}
