import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api, b64ToBytes, events, type EditorContext, type ProjectInfo, type Section } from "./api";
import { ClaudePanel } from "./claude";
import { Editors } from "./editor";
import { PANELS, renderPanel, type PanelId } from "./panels";
import { Preview } from "./preview";
import { initTheme, themePicker } from "./themes";
import { setupWizard } from "./setup";
import { authorBadge, clear, form, guard, h, toast } from "./ui";
import { checkForUpdates } from "./updates";
import {
  EDITOR_FONT_DEFAULT,
  EDITOR_FONT_MAX,
  EDITOR_FONT_MIN,
  EDITOR_FONT_STEP,
  applyEditorFont,
  clampEditorFont,
  formatEditorFont,
  loadEditorFont,
  loadSidebarOpen,
  onHideSidebar,
  saveEditorFont,
  saveSidebarOpen,
  viewShortcut,
} from "./view-prefs";

initTheme();

const app = document.getElementById("app")!;
let unlisten: UnlistenFn[] = [];
let syncTimer: number | undefined;
let cleanups: Array<() => void> = [];
function cleanupWorkbench() {
  for (const fn of cleanups) fn();
  cleanups = [];
}

const RECENT_KEY = "lwp.recent";
function recentProjects(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}
function remember(root: string) {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify([root, ...recentProjects().filter((r) => r !== root)].slice(0, 8)));
  } catch {
    /* storage unavailable */
  }
}

// ---- welcome -------------------------------------------------------------------

function welcome() {
  cleanupWorkbench();
  const openExisting = async (path?: string) => {
    const dir = path ?? (await openDialog({ directory: true, title: "Open a paper folder" }));
    if (typeof dir !== "string") return;
    const info = await guard(api.openProject(dir));
    if (info) await workbench(info);
  };
  const create = async () => {
    const dir = await openDialog({ directory: true, title: "Choose a folder for the paper (existing .tex/.typ files are imported)" });
    if (typeof dir !== "string") return;
    const v = await form("New paper", [
      { name: "title", label: "Title", value: dir.split(/[\\/]/).pop() ?? "Untitled", required: true },
      { name: "format", label: "Format", type: "select", options: [["latex", "LaTeX"], ["typst", "Typst (instant preview)"]] },
    ], "Create");
    if (!v) return;
    const info = await guard(api.initProject(dir, v.title, v.format as "latex" | "typst"));
    if (info) await workbench(info);
  };
  const joinShared = async () => {
    const v = await form("Join a shared paper", [{ name: "link", label: "Share link", placeholder: "http://…/p/…#…", required: true }], "Choose folder…");
    if (!v) return;
    const dir = await openDialog({ directory: true, title: "Choose an empty folder for the paper" });
    if (typeof dir !== "string") return;
    const info = await guard(api.joinProject(v.link.trim(), dir));
    if (info) await workbench(info);
  };
  const recent = recentProjects();
  clear(
    app,
    h(
      "div",
      { class: "welcome" },
      h("h1", {}, "LetsWritePaper"),
      h("p", { class: "muted" }, "Write papers with co-authors and AI agents. Every edit is versioned and attributed."),
      h("div", { class: "welcome-actions" },
        h("button", { class: "primary big", onclick: create }, "New paper…"),
        h("button", { class: "big", onclick: () => openExisting() }, "Open paper…"),
        h("button", { class: "big", onclick: joinShared }, "Join a shared paper…"),
      ),
      h("button", { class: "link", onclick: () => void setupWizard(welcome) }, "Set up this computer…"),
      recent.length
        ? h("div", { class: "recent" }, h("h4", {}, "Recent"), ...recent.map((r) => h("button", { class: "link", onclick: () => openExisting(r) }, r)))
        : null,
      themePicker(true),
    ),
  );
}

// ---- workbench -------------------------------------------------------------------

async function workbench(info: ProjectInfo) {
  cleanupWorkbench();
  unlisten.forEach((u) => u());
  unlisten = [];
  remember(info.root);
  document.title = `${info.title} — LetsWritePaper`;

  const activity = h("nav", { class: "activitybar", "aria-label": "Panels" });
  let sidebarOpen = loadSidebarOpen();
  const sidebar = h("aside", {
    class: sidebarOpen ? "sidebar" : "sidebar hidden",
    "aria-label": "Sidebar",
  });
  const toggleSidebarBtn = h("button", {
    class: "icon sidebar-toggle",
    "aria-keyshortcuts": "Control+B Meta+B",
    onclick: () => setSidebar(!sidebarOpen),
  }, "◀");
  const tabs = h("div", { class: "tabs", role: "tablist" });
  const editorArea = h("div", { class: "editor-area" });
  const empty = h("div", { class: "empty-editor muted" }, "Open a document from the sidebar.");
  editorArea.append(empty);
  const previewEl = h("section", { class: "preview" });
  const claudeEl = h("section", { class: "claude hidden" });
  const rightTab = (id: "preview" | "claude", label: string) =>
    h("button", { class: `rtab ${id === "preview" ? "active" : ""}`, "data-tab": id, onclick: () => showRight(id) }, label);
  const rightTabs = h("div", { class: "rtabs", role: "tablist" }, rightTab("preview", "Preview"), rightTab("claude", "◆ Assistant"));
  const rightPane = h("div", { class: "rightpane" }, rightTabs, previewEl, claudeEl);
  const toggleRight = () => rightPane.classList.toggle("hidden");
  let editorFont = loadEditorFont();
  applyEditorFont(editorFont);
  const fontSmaller = h("button", {
    class: "icon",
    title: "Smaller editor text (⌘/Ctrl+−)",
    "aria-label": "Smaller editor text",
    "aria-keyshortcuts": "Control+- Meta+-",
    disabled: editorFont <= EDITOR_FONT_MIN,
    onclick: () => setFont(editorFont - EDITOR_FONT_STEP),
  }, "−");
  const fontReadout = h("button", {
    class: "icon font-readout",
    title: "Reset editor text (⌘/Ctrl+0)",
    "aria-label": "Reset editor text",
    "aria-keyshortcuts": "Control+0 Meta+0",
    onclick: () => setFont(EDITOR_FONT_DEFAULT),
  }, formatEditorFont(editorFont));
  const fontLarger = h("button", {
    class: "icon",
    title: "Larger editor text (⌘/Ctrl+=)",
    "aria-label": "Larger editor text",
    "aria-keyshortcuts": "Control+= Meta+=",
    disabled: editorFont >= EDITOR_FONT_MAX,
    onclick: () => setFont(editorFont + EDITOR_FONT_STEP),
  }, "+");
  const fontControls = h("span", { class: "font-controls" }, fontSmaller, fontReadout, fontLarger);
  const status = {
    sync: h("span", {}),
    cursor: h("span", {}),
    words: h("span", {}),
    compile: h("span", {}),
  };
  const statusbar = h(
    "footer",
    { class: "statusbar" },
    authorBadge(info.me),
    status.sync,
    h("span", { class: "spacer" }),
    status.words,
    status.cursor,
    status.compile,
    fontControls,
    themePicker(),
  );
  const togglePreview = h("button", {
    class: "icon",
    title: "Toggle side pane (⌘/Ctrl+\\)",
    "aria-label": "Toggle side pane",
    onclick: () => toggleRight(),
  }, "◧");
  const claudeBtn = h("button", { class: "icon", title: "Ask an assistant (⌘/Ctrl+L tags this section)", onclick: () => showRight("claude") }, "◆");
  const main = h(
    "main",
    { class: "main" },
    h("div", { class: "tabbar" }, toggleSidebarBtn, h("span", { class: "tabbar-sep" }), tabs, h("span", { class: "spacer" }), claudeBtn, togglePreview),
    editorArea,
  );
  clear(app, h("div", { class: "workbench" }, activity, sidebar, main, rightPane), statusbar);

  const editors = new Editors(tabs, editorArea, info.me);
  const setFont = (size: number) => {
    editorFont = clampEditorFont(size);
    applyEditorFont(editorFont);
    saveEditorFont(editorFont);
    fontReadout.textContent = formatEditorFont(editorFont);
    fontSmaller.disabled = editorFont <= EDITOR_FONT_MIN;
    fontLarger.disabled = editorFont >= EDITOR_FONT_MAX;
    editors.remeasure();
  };
  const preview = new Preview(previewEl);
  let outlineCache: { path: string; sections: Section[] } | null = null;
  let cursorLine = 1;
  const editorContext = (): EditorContext => {
    const path = editors.active ?? undefined;
    const sections = outlineCache && outlineCache.path === path ? outlineCache.sections : [];
    const section = sections.filter((s) => s.line <= cursorLine && cursorLine <= s.end_line).sort((a, b) => b.level - a.level)[0];
    const selection = editors.selectionText();
    return { path, line: path ? cursorLine : undefined, section: section?.title, selection: selection || undefined };
  };
  const claude = new ClaudePanel(claudeEl, editorContext, info.title, () => editors.listDocs());
  const showRight = (id: "preview" | "claude") => {
    rightPane.classList.remove("hidden");
    previewEl.classList.toggle("hidden", id !== "preview");
    claudeEl.classList.toggle("hidden", id !== "claude");
    rightTabs.querySelectorAll(".rtab").forEach((b) => b.classList.toggle("active", (b as HTMLElement).dataset.tab === id));
    if (id === "claude") void claude.show();
  };
  const refreshOutlineCache = async (path: string | null) => {
    if (!path) return;
    const sections = await api.outline(path).catch(() => [] as Section[]);
    outlineCache = { path, sections };
  };
  const isTypst = info.format === "typst";
  let panel: PanelId = "files";

  const refresh = () => void renderPanel(panel, sidebar, ctx).catch((e) => toast(String(e), "error"));
  const ctx = {
    info,
    editors,
    refresh,
    openDoc: async (path: string, line?: number) => {
      await guard(editors.open(path, line));
    },
  };
  editors.listDocs = () => ctx.info.documents;

  const renderActivity = () =>
    clear(
      activity,
      ...PANELS.map((p) =>
        h("button", {
          class: `activity ${p.id === panel ? "active" : ""}`,
          title: p.id !== panel ? p.label : sidebarOpen ? `${p.label} — hide sidebar (⌘/Ctrl+B)` : `${p.label} — show sidebar (⌘/Ctrl+B)`,
          "aria-label": p.label,
          "aria-expanded": p.id === panel && sidebarOpen ? "true" : "false",
          onclick: () => {
            if (p.id === panel && sidebarOpen) {
              setSidebar(false);
              return;
            }
            panel = p.id;
            if (!sidebarOpen) setSidebar(true);
            else renderActivity();
            refresh();
          },
        }, p.icon),
      ),
      h("span", { class: "spacer" }),
      h("button", {
        class: "activity",
        title: "Close project",
        "aria-label": "Close project",
        onclick: async () => {
          await editors.flushAll();
          await claude.stop();
          editors.closeAll();
          welcome();
        },
      }, "⏏"),
    );
  const paintChrome = () => {
    const sidebarLabel = sidebarOpen ? "Hide sidebar" : "Show sidebar";
    toggleSidebarBtn.textContent = sidebarOpen ? "◀" : "▶";
    toggleSidebarBtn.title = `${sidebarLabel} (⌘/Ctrl+B)`;
    toggleSidebarBtn.setAttribute("aria-label", sidebarLabel);
    toggleSidebarBtn.setAttribute("aria-pressed", sidebarOpen ? "true" : "false");
    empty.textContent = sidebarOpen ? "Open a document from the sidebar." : "Sidebar is hidden — ⌘B or Ctrl+B shows it.";
  };
  const setSidebar = (open: boolean) => {
    sidebarOpen = open;
    sidebar.classList.toggle("hidden", !open);
    sidebar.toggleAttribute("aria-hidden", !open);
    saveSidebarOpen(open);
    paintChrome();
    renderActivity();
  };
  setSidebar(sidebarOpen);

  const updateSync = async () => {
    const share = await api.shareInfo().catch(() => null);
    if (share?.shared) {
      const st = share.status;
      status.sync.textContent = st?.connected ? "⇄ synced across devices" : "⇄ sync offline";
      status.sync.title = st?.last_error ?? share.link ?? "";
      return;
    }
    const i = await api.projectInfo().catch(() => null);
    status.sync.textContent = i?.owner ? "● synced to disk" : "○ following another process";
    status.sync.title = i?.owner
      ? "This window mirrors documents to their files and records outside edits."
      : "Another LetsWritePaper process mirrors files; edits still sync live.";
  };
  void updateSync();
  window.clearInterval(syncTimer);
  syncTimer = window.setInterval(() => void updateSync(), 3000);

  editors.onActivate = (path) => {
    empty.classList.toggle("hidden", !!path);
    if (panel === "outline" || panel === "files") refresh();
    updateWords(path);
    void refreshOutlineCache(path);
  };
  const updateWords = (path: string | null) => {
    const text = path ? editors.text(path) : null;
    status.words.textContent = text === null ? "" : `${text.split(/\s+/).filter(Boolean).length} words`;
  };
  let outlineTimer: number | undefined;
  let historyTimer: number | undefined;
  const refreshHistory = () => {
    if (panel !== "history") return;
    window.clearTimeout(historyTimer);
    historyTimer = window.setTimeout(async () => {
      await editors.flushAll();
      if (panel === "history") refresh();
    }, 700);
  };
  editors.onChange = (path) => {
    updateWords(path);
    if (isTypst) preview.schedule(700);
    window.clearTimeout(outlineTimer);
    outlineTimer = window.setTimeout(() => {
      void refreshOutlineCache(path);
      if (panel === "outline") refresh();
    }, 1200);
    refreshHistory();
  };
  editors.onCursor = (_path, line) => {
    cursorLine = line;
    status.cursor.textContent = `Ln ${line}`;
  };

  preview.onStatus = (text, ok) => {
    status.compile.textContent = text;
    status.compile.className = ok === null ? "" : ok ? "ok" : "bad";
  };
  preview.onDiagnosticClick = (d) => {
    if (d.file && info.documents.includes(d.file)) void ctx.openDoc(d.file, d.line ?? undefined);
    else if (d.line && editors.active) editors.goto(d.line);
  };
  preview.beforeCompile = () => editors.flushAll();
  const compileNow = () => preview.compile();

  cleanups.push(onHideSidebar(() => setSidebar(false)));
  let wheelPending = 0;
  let wheelTimer: number | undefined;
  const onKey = (e: KeyboardEvent) => {
    if (e.isComposing || e.key === "Process") return;
    if (document.querySelector(".overlay")) return;
    if ((e.metaKey || e.ctrlKey) && e.key === "s") {
      e.preventDefault();
      void compileNow();
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
      e.preventDefault();
      showRight("claude");
      if (!e.shiftKey) void refreshOutlineCache(editors.active).then(() => claude.tagCursorSection());
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "b") {
      e.preventDefault();
      void editors.toggleBlame();
      return;
    }
    const shortcut = viewShortcut(e);
    if (!shortcut) return;
    e.preventDefault();
    switch (shortcut) {
      case "sidebar":
        setSidebar(!sidebarOpen);
        break;
      case "right-pane":
        toggleRight();
        break;
      case "font-up":
        setFont(editorFont + EDITOR_FONT_STEP);
        break;
      case "font-down":
        setFont(editorFont - EDITOR_FONT_STEP);
        break;
      case "font-reset":
        setFont(EDITOR_FONT_DEFAULT);
        break;
    }
  };
  const onWheel = (e: WheelEvent) => {
    if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
    if (document.querySelector(".overlay")) return;
    e.preventDefault();
    const dy =
      e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaY * 16 : e.deltaMode === WheelEvent.DOM_DELTA_PAGE ? e.deltaY * 400 : e.deltaY;
    wheelPending += dy;
    window.clearTimeout(wheelTimer);
    wheelTimer = window.setTimeout(() => {
      wheelPending = 0;
    }, 180);
    const stepPx = 60;
    if (Math.abs(wheelPending) < stepPx) return;
    const steps = Math.trunc(wheelPending / stepPx);
    wheelPending -= steps * stepPx;
    const dir = steps > 0 ? -1 : 1;
    const count = Math.min(Math.abs(steps), 3);
    const overPreview =
      !previewEl.classList.contains("hidden") && !rightPane.classList.contains("hidden") && e.target instanceof Node && previewEl.contains(e.target);
    if (overPreview) preview.nudgeZoom(dir * 0.1 * count);
    else setFont(editorFont + dir * count);
  };
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: false });
  cleanups.push(() => {
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("wheel", onWheel, true);
    window.clearTimeout(wheelTimer);
  });

  // Live events from the workspace (agents, other windows, files on disk).
  let metaTimer: number | undefined;
  const blameTimers = new Map<string, number>();
  unlisten.push(
    await events.onDocUpdate(({ path, update, author }) => {
      editors.applyRemote(path, b64ToBytes(update));
      if (isTypst) preview.schedule(400);
      if (author.kind === "agent" && path === editors.active) status.sync.textContent = `◆ ${author.name} is editing`;
      if (editors.blameEnabled) {
        window.clearTimeout(blameTimers.get(path));
        blameTimers.set(path, window.setTimeout(() => void editors.refreshBlame(path), 800));
      }
      refreshHistory();
    }),
    await events.onDocAdded(() => {
      void api.projectInfo().then((i) => (ctx.info.documents = i.documents));
      if (panel === "files") refresh();
    }),
    await events.onDocRemoved((path) => {
      editors.close(path);
      if (panel === "files") refresh();
    }),
    await events.onMetaChanged(() => {
      window.clearTimeout(metaTimer);
      metaTimer = window.setTimeout(async () => {
        void updateSync();
        if (panel !== "files") refresh();
        const comments = await api.reviewComments({ status: "open" }).catch(() => []);
        editors.setReviewComments(comments);
      }, 300);
    }),
  );

  refresh();
  if (info.documents.includes(info.main)) await ctx.openDoc(info.main);
  const comments = await api.reviewComments({ status: "open" }).catch(() => []);
  editors.setReviewComments(comments);
  void compileNow();
}

function openOrWelcome() {
  void api.projectInfo().then(workbench).catch(() => welcome());
}

// First launch runs the setup wizard. A project path on the command line
// (`letswritepaper ~/papers/foo`) still opens once setup finishes.
void api
  .setupStatus()
  .then((status) => {
    if (!status.setup_done) void setupWizard(openOrWelcome);
    else openOrWelcome();
  })
  .catch(() => openOrWelcome());

void checkForUpdates();
