import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api, b64ToBytes, events, type EditorContext, type ProjectInfo, type Section } from "./api";
import { ClaudePanel } from "./claude";
import { Editors } from "./editor";
import { PANELS, renderPanel, type PanelId } from "./panels";
import { Preview } from "./preview";
import { authorBadge, clear, form, guard, h, toast } from "./ui";

const app = document.getElementById("app")!;
let unlisten: UnlistenFn[] = [];
let syncTimer: number | undefined;

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
      recent.length
        ? h("div", { class: "recent" }, h("h4", {}, "Recent"), ...recent.map((r) => h("button", { class: "link", onclick: () => openExisting(r) }, r)))
        : null,
    ),
  );
}

// ---- workbench -------------------------------------------------------------------

async function workbench(info: ProjectInfo) {
  unlisten.forEach((u) => u());
  unlisten = [];
  remember(info.root);
  document.title = `${info.title} — LetsWritePaper`;

  const activity = h("nav", { class: "activitybar", "aria-label": "Panels" });
  const sidebar = h("aside", { class: "sidebar" });
  const tabs = h("div", { class: "tabs", role: "tablist" });
  const editorArea = h("div", { class: "editor-area" });
  const empty = h("div", { class: "empty-editor muted" }, "Open a document from the sidebar.");
  editorArea.append(empty);
  const previewEl = h("section", { class: "preview" });
  const claudeEl = h("section", { class: "claude hidden" });
  const rightTab = (id: "preview" | "claude", label: string) =>
    h("button", { class: `rtab ${id === "preview" ? "active" : ""}`, "data-tab": id, onclick: () => showRight(id) }, label);
  const rightTabs = h("div", { class: "rtabs", role: "tablist" }, rightTab("preview", "Preview"), rightTab("claude", "◆ Claude"));
  const rightPane = h("div", { class: "rightpane" }, rightTabs, previewEl, claudeEl);
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
  );
  const togglePreview = h("button", { class: "icon", title: "Toggle side pane", onclick: () => rightPane.classList.toggle("hidden") }, "◧");
  const claudeBtn = h("button", { class: "icon", title: "Ask Claude (⌘/Ctrl+L)", onclick: () => showRight("claude") }, "◆");
  const main = h("main", { class: "main" }, h("div", { class: "tabbar" }, tabs, h("span", { class: "spacer" }), claudeBtn, togglePreview), editorArea);
  clear(app, h("div", { class: "workbench" }, activity, sidebar, main, rightPane), statusbar);

  const editors = new Editors(tabs, editorArea, info.me);
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
  const claude = new ClaudePanel(claudeEl, editorContext, info.title);
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

  const renderActivity = () =>
    clear(
      activity,
      ...PANELS.map((p) =>
        h("button", {
          class: `activity ${p.id === panel ? "active" : ""}`,
          title: p.label,
          "aria-label": p.label,
          onclick: () => {
            panel = p.id;
            renderActivity();
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
  renderActivity();

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
  editors.onChange = (path) => {
    updateWords(path);
    if (isTypst) preview.schedule(700);
    window.clearTimeout(outlineTimer);
    outlineTimer = window.setTimeout(() => {
      void refreshOutlineCache(path);
      if (panel === "outline") refresh();
    }, 1200);
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

  window.onkeydown = (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "s") {
      e.preventDefault();
      void compileNow();
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
      e.preventDefault();
      showRight("claude");
    }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "b") {
      e.preventDefault();
      void editors.toggleBlame();
    }
  };

  refresh();
  if (info.documents.includes(info.main)) await ctx.openDoc(info.main);
  const comments = await api.reviewComments({ status: "open" }).catch(() => []);
  editors.setReviewComments(comments);
  void compileNow();
}

// Opened with a project path (e.g. `letswritepaper ~/papers/foo`)?
api
  .projectInfo()
  .then(workbench)
  .catch(() => welcome());
