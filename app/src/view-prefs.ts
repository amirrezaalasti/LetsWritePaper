// View preferences for the workbench: sidebar visibility and editor font size.

const SIDEBAR_KEY = "lwp.sidebar";
const FONT_KEY = "lwp.editorFont";

export const EDITOR_FONT_DEFAULT = 13.5;
export const EDITOR_FONT_MIN = 10;
export const EDITOR_FONT_MAX = 28;
export const EDITOR_FONT_STEP = 1;

export function clampEditorFont(size: number): number {
  const stepped = Math.round(size * 2) / 2;
  return Math.min(EDITOR_FONT_MAX, Math.max(EDITOR_FONT_MIN, stepped));
}

export function loadEditorFont(): number {
  try {
    const n = Number(localStorage.getItem(FONT_KEY));
    if (Number.isFinite(n) && n > 0) return clampEditorFont(n);
  } catch {
    /* storage unavailable */
  }
  return EDITOR_FONT_DEFAULT;
}

export function saveEditorFont(size: number) {
  try {
    localStorage.setItem(FONT_KEY, String(size));
  } catch {
    /* storage unavailable */
  }
}

export function applyEditorFont(size: number) {
  document.documentElement.style.setProperty("--editor-font", `${size}px`);
}

export function formatEditorFont(size: number): string {
  const text = Number.isInteger(size) ? String(size) : size.toFixed(1);
  return `Aa ${text}`;
}

export function loadSidebarOpen(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_KEY) !== "0";
  } catch {
    return true;
  }
}

export function saveSidebarOpen(open: boolean) {
  try {
    localStorage.setItem(SIDEBAR_KEY, open ? "1" : "0");
  } catch {
    /* storage unavailable */
  }
}

let hideSidebarHandler: (() => void) | undefined;

/** Panel headers call this. The workbench registers the handler. */
export function requestHideSidebar() {
  hideSidebarHandler?.();
}

export function onHideSidebar(handler: () => void): () => void {
  hideSidebarHandler = handler;
  return () => {
    if (hideSidebarHandler === handler) hideSidebarHandler = undefined;
  };
}

export type ViewShortcut = "sidebar" | "right-pane" | "font-up" | "font-down" | "font-reset";

/** Shortcuts that use ⌘/Ctrl. Existing compile, assistant, and blame chords are left to the caller. */
export function viewShortcut(e: KeyboardEvent): ViewShortcut | null {
  if (e.isComposing || e.key === "Process") return null;
  if (!(e.metaKey || e.ctrlKey) || e.altKey) return null;
  const key = e.key.toLowerCase();
  if (!e.shiftKey && key === "b") return "sidebar";
  if (!e.shiftKey && (key === "\\" || e.code === "Backslash")) return "right-pane";
  if (e.code === "NumpadAdd" || key === "+" || key === "=" || e.code === "Equal") return "font-up";
  if (e.code === "NumpadSubtract" || key === "-" || key === "_" || e.code === "Minus") return "font-down";
  if (!e.shiftKey && (key === "0" || e.code === "Numpad0")) return "font-reset";
  return null;
}
