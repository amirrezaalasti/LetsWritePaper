// Color themes. The choice is stored as lwp.theme; "system" follows the OS
// and resolves to Ink or Paper. index.html sets the attribute before first paint.
import { h } from "./ui";

export const THEME_KEY = "lwp.theme";

export const THEMES = [
  { id: "system", label: "System", hint: "Follows this computer", swatch: ["#1e1f22", "#f6f7f9"] },
  { id: "ink", label: "Ink", hint: "Charcoal", swatch: ["#1e1f22", "#5b8cff"] },
  { id: "paper", label: "Paper", hint: "White page", swatch: ["#ffffff", "#2f6bf0"] },
  { id: "sepia", label: "Sepia", hint: "Warm manuscript", swatch: ["#f6f0e4", "#8a4b2f"] },
  { id: "fog", label: "Fog", hint: "Cool gray", swatch: ["#f3f6f8", "#1f7a8c"] },
  { id: "midnight", label: "Midnight", hint: "Deep navy", swatch: ["#0e1520", "#7eb6ff"] },
  { id: "grove", label: "Grove", hint: "Library green", swatch: ["#141c18", "#c6a15a"] },
] as const;

export type ThemeId = (typeof THEMES)[number]["id"];

const NAMED = new Set<string>(THEMES.map((t) => t.id));

function isTheme(id: string | null): id is ThemeId {
  return !!id && NAMED.has(id);
}

export function themeChoice(): ThemeId {
  try {
    const stored = localStorage.getItem(THEME_KEY);
    if (isTheme(stored)) return stored;
  } catch {
    /* storage unavailable */
  }
  return "system";
}

function prefersLight(): boolean {
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

/** Theme actually painted. System becomes Ink or Paper. */
export function resolvedTheme(choice = themeChoice()): Exclude<ThemeId, "system"> {
  if (choice === "system") return prefersLight() ? "paper" : "ink";
  return choice;
}

export function applyTheme(choice: ThemeId) {
  try {
    localStorage.setItem(THEME_KEY, choice);
  } catch {
    /* storage unavailable */
  }
  const theme = resolvedTheme(choice);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.themeChoice = choice;
  document.querySelectorAll<HTMLElement>(".theme-wrap").forEach((wrap) => {
    wrap.dispatchEvent(new Event("themechange"));
  });
}

export function initTheme() {
  applyTheme(themeChoice());
  const media = window.matchMedia("(prefers-color-scheme: light)");
  const onChange = () => {
    if (themeChoice() === "system") applyTheme("system");
  };
  media.addEventListener?.("change", onChange);
}

function swatch(colors: readonly [string, string]) {
  return h("span", { class: "swatch", style: `--swatch-a:${colors[0]};--swatch-b:${colors[1]}` });
}

/** Menu of themes. `dock` pins it to the welcome screen; otherwise it sits in the status bar. */
export function themePicker(dock = false): HTMLElement {
  const menu = h("div", { class: "theme-menu hidden", role: "menu" });
  const button = h(
    "button",
    {
      class: "theme-picker",
      type: "button",
      title: "Color theme",
      "aria-haspopup": "menu",
      "aria-expanded": "false",
    },
    swatch(THEMES[0].swatch),
    h("span", { class: "theme-picker-label" }, "Theme"),
  );
  const wrap = h("div", { class: dock ? "theme-wrap theme-dock" : "theme-wrap" }, button, menu);

  const close = () => {
    menu.classList.add("hidden");
    button.setAttribute("aria-expanded", "false");
  };
  const paint = () => {
    const choice = themeChoice();
    const meta = THEMES.find((t) => t.id === choice) ?? THEMES[0];
    button.replaceChildren(swatch(meta.swatch), h("span", { class: "theme-picker-label" }, meta.label));
    menu.replaceChildren(
      ...THEMES.map((t) =>
        h(
          "button",
          {
            class: "theme-option",
            type: "button",
            role: "menuitemradio",
            "data-theme-id": t.id,
            "aria-checked": t.id === choice ? "true" : "false",
            onclick: () => {
              applyTheme(t.id);
              close();
            },
          },
          swatch(t.swatch),
          h("span", { class: "theme-option-text" }, h("span", {}, t.label), h("span", { class: "muted" }, t.hint)),
        ),
      ),
    );
  };

  let onDoc: ((ev: MouseEvent) => void) | null = null;
  const unhook = () => {
    if (!onDoc) return;
    document.removeEventListener("mousedown", onDoc);
    onDoc = null;
  };
  button.addEventListener("click", (e) => {
    e.stopPropagation();
    const opening = menu.classList.contains("hidden");
    menu.classList.toggle("hidden");
    button.setAttribute("aria-expanded", opening ? "true" : "false");
    unhook();
    if (!opening) return;
    onDoc = (ev) => {
      if (wrap.contains(ev.target as Node)) return;
      close();
      unhook();
    };
    document.addEventListener("mousedown", onDoc);
  });
  wrap.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      close();
      unhook();
      button.focus();
    }
  });
  wrap.addEventListener("themechange", paint);
  paint();
  return wrap;
}
