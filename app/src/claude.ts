// The Claude panel: chat with Claude Code about the paper. Claude edits
// through the paper's MCP tools, so its changes stream into the editor and
// history like a co-author's.
import { api, events, type ClaudeSession, type EditorContext } from "./api";
import { clear, h, showModal, timeAgo, toast } from "./ui";

/** Minimal, safe Markdown: everything is escaped before formatting is applied. */
export function renderMarkdown(src: string): string {
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const inline = (s: string) =>
    esc(s)
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/(^|[\s(])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>");
  const out: string[] = [];
  const parts = src.split(/```/);
  parts.forEach((part, i) => {
    if (i % 2 === 1) {
      const body = part.replace(/^[\w-]*\n/, "");
      out.push(`<pre class="md-code"><code>${esc(body)}</code></pre>`);
      return;
    }
    let list: string[] = [];
    const flush = () => {
      if (list.length) out.push(`<ul>${list.map((l) => `<li>${inline(l)}</li>`).join("")}</ul>`);
      list = [];
    };
    for (const block of part.split(/\n{2,}/)) {
      const lines = block.split("\n").filter((l) => l.trim());
      if (!lines.length) continue;
      if (lines.every((l) => /^\s*([-*]|\d+\.)\s+/.test(l))) {
        list.push(...lines.map((l) => l.replace(/^\s*([-*]|\d+\.)\s+/, "")));
        continue;
      }
      flush();
      const head = lines[0].match(/^(#{1,4})\s+(.*)/);
      if (head && lines.length === 1) out.push(`<h${head[1].length + 2}>${inline(head[2])}</h${head[1].length + 2}>`);
      else out.push(`<p>${lines.map(inline).join("<br>")}</p>`);
    }
    flush();
  });
  return out.join("");
}

const QUICK_ACTIONS: [string, string][] = [
  ["Improve selection", "Improve the selected text for clarity and concision. Keep the meaning, citations, labels and math. Apply it with edit_document."],
  ["Review this section", "Read the section I'm in with section_context and give me the three most important problems, as review comments (start_review, add_review_comment, finish_review). Don't edit."],
  ["Address open comments", "Work through the open review comments on this paper: fix what you can with edit_document (or propose_edit for big changes) and record a response for each with respond_to_comment."],
  ["Mock peer review", "Run a mock peer review of the whole paper as the methods-skeptic persona, following the peer-review skill."],
  ["Check it compiles", "Compile the paper and fix any errors you find."],
];

interface ToolRow {
  el: HTMLElement;
  status: HTMLElement;
  detail: HTMLElement;
}

export class ClaudePanel {
  private chatId = crypto.randomUUID();
  private sessionId: string | null = null;
  private running = false;
  private busy = false;
  private mode: "paper" | "edits" = "paper";
  private log: HTMLElement;
  private input: HTMLTextAreaElement;
  private sendBtn: HTMLButtonElement;
  private statusLine: HTMLElement;
  private sessionSelect: HTMLSelectElement;
  private shareContext = true;
  private bubbles = new Map<string, { el: HTMLElement; text: string; streamed: boolean }>();
  private tools = new Map<string, ToolRow>();
  private currentMsg: string | null = null;
  private renderQueued = new Set<string>();
  private stderr: string[] = [];
  private ready = false;

  constructor(
    private root: HTMLElement,
    private context: () => EditorContext,
    private paperTitle: string,
  ) {
    this.log = h("div", { class: "chat-log", "aria-live": "polite" });
    this.input = h("textarea", { class: "chat-input", rows: 3, placeholder: "Ask Claude to draft, revise, review… (Enter to send, Shift+Enter for a new line)" });
    this.sendBtn = h("button", { class: "primary", onclick: () => void this.submit() }, "Send");
    this.statusLine = h("span", { class: "muted small" }, "");
    this.sessionSelect = h("select", { class: "small-select", "aria-label": "Chat session", onchange: () => void this.switchSession() });
    this.input.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        void this.submit();
      }
    });
    void events.onClaude(({ chat_id, event }) => {
      if (chat_id === this.chatId) this.onEvent(event);
    });
  }

  /** Called when the panel becomes visible. */
  async show() {
    if (this.ready) return;
    const status = await api.claudeStatus().catch(() => null);
    if (!status?.installed) return this.renderInstall(status?.install_command ?? "");
    this.ready = true;
    this.renderChat(status.version ?? "");
    await this.loadSessions();
  }

  private renderInstall(command: string) {
    const out = h("pre", { class: "install-log" });
    const btn = h("button", {
      class: "primary",
      onclick: async () => {
        btn.disabled = true;
        btn.textContent = "Installing…";
        const unlisten = await events.onInstallLog((line) => {
          out.textContent += line + "\n";
          out.scrollTop = out.scrollHeight;
        });
        try {
          const s = await api.claudeInstall();
          if (s.installed) {
            toast(`Claude Code ${s.version ?? ""} installed`);
            this.ready = false;
            await this.show();
          }
        } catch (e) {
          toast(String(e), "error");
          btn.disabled = false;
          btn.textContent = "Retry install";
        } finally {
          unlisten();
        }
      },
    }, "Install Claude Code");
    clear(
      this.root,
      h(
        "div",
        { class: "claude-setup" },
        h("h3", {}, "Claude in your paper"),
        h("p", {}, "This panel runs Claude Code (Anthropic's agent) with access to your paper's tools: it drafts, edits, reviews and compiles, and every change shows up live with its name in the history."),
        h("p", { class: "muted" }, "Claude Code isn't installed yet. The official installer runs:"),
        h("code", { class: "block" }, command),
        h("div", { class: "actions" }, btn, h("button", { onclick: () => ((this.ready = false), void this.show()) }, "I installed it")),
        out,
        h("p", { class: "muted small" }, "You'll sign in once with your Claude account (Pro, Max, Team or Enterprise) or an Anthropic API key."),
      ),
    );
  }

  private renderChat(version: string) {
    const modeSelect = h("select", {
      class: "small-select",
      "aria-label": "What Claude may do",
      title: "Paper tools: Claude edits only through the paper's tools (recommended). File edits: Claude may also edit files and use its other tools.",
      onchange: async () => {
        this.mode = modeSelect.value as "paper" | "edits";
        if (this.running) await this.restart();
      },
    }, h("option", { value: "paper" }, "Paper tools"), h("option", { value: "edits" }, "Paper tools + file edits"));
    const ctxToggle = h("label", { class: "check", title: "Send the open file, section and selection with each message" },
      (() => {
        const cb = h("input", { type: "checkbox" });
        cb.checked = this.shareContext;
        cb.onchange = () => (this.shareContext = cb.checked);
        return cb;
      })(),
      "Editor context",
    );
    clear(
      this.root,
      h(
        "div",
        { class: "pane-header" },
        h("strong", {}, "Claude"),
        this.sessionSelect,
        h("span", { class: "spacer" }),
        h("button", { class: "icon", title: "Continue on your phone or another computer", "aria-label": "Continue on another device", onclick: () => this.continueElsewhere() }, "📱"),
        h("button", { class: "icon", title: "Open this chat in a terminal", "aria-label": "Open in terminal", onclick: () => void this.openTerminal("resume") }, "⌨"),
        h("button", { class: "icon", title: "Sign in to Claude", "aria-label": "Sign in", onclick: () => void this.openTerminal("login") }, "🔑"),
      ),
      this.log,
      h(
        "div",
        { class: "chat-compose" },
        h("div", { class: "chips" }, ...QUICK_ACTIONS.map(([label, prompt]) => h("button", { class: "chip add", onclick: () => void this.submit(prompt) }, label))),
        this.input,
        h("div", { class: "chat-bar" }, modeSelect, ctxToggle, this.statusLine, h("span", { class: "spacer" }), this.sendBtn),
      ),
    );
    this.statusLine.textContent = version ? `Claude Code ${version.split(" ")[0]}` : "";
    if (!this.log.childElementCount) {
      this.log.append(
        h("div", { class: "chat-empty muted" },
          h("p", {}, "Ask Claude to work on your paper. It sees which file and section you're in and what you've selected."),
          h("p", {}, "Its edits appear live in the editor, credited to claude-code in History and Blame."),
        ),
      );
    }
  }

  private async loadSessions(selected?: string) {
    const sessions = await api.claudeSessions().catch(() => [] as ClaudeSession[]);
    clear(
      this.sessionSelect,
      h("option", { value: "" }, "New chat"),
      ...sessions.map((s) => h("option", { value: s.id }, `${s.title || s.id.slice(0, 8)} · ${timeAgo(s.updated_at)}`)),
    );
    this.sessionSelect.value = selected ?? this.sessionId ?? "";
  }

  private async switchSession() {
    const id = this.sessionSelect.value || null;
    await this.stop();
    this.sessionId = id;
    this.chatId = crypto.randomUUID();
    this.bubbles.clear();
    this.tools.clear();
    clear(this.log, h("div", { class: "chat-note muted" }, id ? "Resuming an earlier chat — Claude remembers it; earlier messages aren't shown here." : "New chat."));
  }

  private async restart() {
    await this.stop();
    await this.ensureRunning();
  }

  private async ensureRunning() {
    if (this.running) return;
    await api.claudeStart(this.chatId, this.mode, this.sessionId ?? undefined);
    this.running = true;
  }

  async stop() {
    if (!this.running) return;
    this.running = false;
    this.setBusy(false);
    await api.claudeStop(this.chatId).catch(() => null);
  }

  private setBusy(b: boolean) {
    this.busy = b;
    this.sendBtn.textContent = b ? "Stop" : "Send";
    this.sendBtn.classList.toggle("primary", !b);
    this.sendBtn.onclick = b ? () => void this.interrupt() : () => void this.submit();
  }

  private async interrupt() {
    await this.stop();
    this.note("Stopped. Send another message to continue the same chat.");
  }

  async submit(text?: string) {
    const msg = (text ?? this.input.value).trim();
    if (!msg || this.busy) return;
    if (!this.ready) await this.show();
    this.input.value = "";
    this.log.querySelector(".chat-empty")?.remove();
    this.log.append(h("div", { class: "msg user" }, msg));
    this.scroll();
    this.stderr = [];
    this.setBusy(true);
    try {
      await this.ensureRunning();
      await api.claudeSend(this.chatId, msg, this.shareContext ? this.context() : undefined);
    } catch (e) {
      this.setBusy(false);
      this.error(String(e));
    }
  }

  /** Hand the chat over to Claude Code's Remote Control in a terminal. */
  private continueElsewhere() {
    showModal(
      "Continue on another device",
      h(
        "div",
        {},
        h("p", {}, "This opens the chat in a terminal with Claude Code's Remote Control. Scan the QR code it shows (press space) or open claude.ai/code on your phone, tablet or another computer to keep talking to Claude, which keeps editing this paper here."),
        h("p", { class: "muted" }, "Requires signing in with a Claude Pro, Max, Team or Enterprise account (not an API key). Keep this computer awake while you use it."),
        h("p", { class: "muted" }, "To edit the paper itself from another device, use Share (⇄) in the sidebar."),
      ),
      [["Open Remote Control", () => void this.openTerminal("remote")]],
    );
  }

  private async openTerminal(action: "login" | "resume" | "remote") {
    try {
      const wasRunning = this.running;
      const cmd = await api.claudeTerminal(action, this.sessionId ?? undefined, action === "login" ? undefined : this.chatId);
      if (action !== "login" && wasRunning) {
        this.running = false;
        this.setBusy(false);
        this.note("This chat continues in the terminal. Send a message here to pick it back up.");
      }
      toast(action === "login" ? "Sign in in the terminal window, then come back." : `Opened: ${cmd.slice(0, 80)}…`);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  // ---- stream handling -------------------------------------------------------------

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onEvent(ev: any) {
    switch (ev.type) {
      case "system":
        if (ev.subtype === "init") {
          this.sessionId = ev.session_id ?? this.sessionId;
          const paper = (ev.mcp_servers ?? []).find((s: { name: string }) => s.name === "paper");
          this.statusLine.textContent = `${ev.model ?? "Claude"}${paper ? ` · paper tools ${paper.status === "connected" ? "✓" : paper.status}` : ""}`;
          void this.loadSessions(this.sessionId ?? undefined);
        }
        break;
      case "stream_event":
        this.onStream(ev.event);
        break;
      case "assistant":
        this.onAssistant(ev.message);
        break;
      case "user":
        for (const block of ev.message?.content ?? []) {
          if (block?.type === "tool_result") this.onToolResult(block);
        }
        break;
      case "result":
        this.setBusy(false);
        this.onResult(ev);
        break;
      case "stderr":
        this.stderr.push(ev.text);
        if (this.stderr.length > 40) this.stderr.shift();
        break;
      case "exit":
        if (this.running) {
          this.running = false;
          this.setBusy(false);
          const hint = this.stderr.join("\n");
          if (/log ?in|auth|credential|api key/i.test(hint)) this.loginNeeded(hint);
          else if (hint.trim()) this.error(hint.split("\n").slice(-6).join("\n"));
        }
        break;
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onStream(e: any) {
    if (e?.type === "message_start") {
      this.currentMsg = e.message?.id ?? null;
    } else if (e?.type === "content_block_delta" && e.delta?.type === "text_delta" && this.currentMsg) {
      const b = this.bubble(this.currentMsg);
      b.text += e.delta.text;
      b.streamed = true;
      this.queueRender(this.currentMsg);
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onAssistant(message: any) {
    const id: string = message?.id ?? crypto.randomUUID();
    for (const block of message?.content ?? []) {
      if (block.type === "text") {
        const b = this.bubble(id);
        if (!b.streamed) {
          b.text = block.text;
          this.queueRender(id);
        }
      } else if (block.type === "tool_use") {
        this.toolRow(block);
      }
    }
  }

  private bubble(id: string) {
    let b = this.bubbles.get(id);
    if (!b) {
      b = { el: h("div", { class: "msg assistant" }), text: "", streamed: false };
      this.bubbles.set(id, b);
      this.log.append(b.el);
    }
    return b;
  }

  private queueRender(id: string) {
    if (this.renderQueued.has(id)) return;
    this.renderQueued.add(id);
    requestAnimationFrame(() => {
      this.renderQueued.delete(id);
      const b = this.bubbles.get(id);
      if (b) b.el.innerHTML = renderMarkdown(b.text);
      this.scroll();
    });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private toolRow(block: any) {
    if (this.tools.has(block.id)) return;
    const name = String(block.name ?? "tool").replace(/^mcp__paper__/, "");
    const input = block.input ?? {};
    const summary = [input.path ?? input.file_path, input.section ?? input.title ?? input.name, input.rationale ?? input.comment]
      .filter(Boolean)
      .map(String)
      .join(" · ");
    const status = h("span", { class: "tool-status" }, "…");
    const detail = h("pre", { class: "tool-detail hidden" }, JSON.stringify(input, null, 2));
    const el = h(
      "div",
      { class: `tool ${block.name?.startsWith("mcp__paper__") ? "paper" : ""}` },
      h("button", { class: "tool-head", onclick: () => detail.classList.toggle("hidden") }, status, h("strong", {}, name), h("span", { class: "muted" }, summary.slice(0, 140))),
      detail,
    );
    this.tools.set(block.id, { el, status, detail });
    this.log.append(el);
    this.scroll();
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onToolResult(block: any) {
    const row = this.tools.get(block.tool_use_id);
    if (!row) return;
    row.status.textContent = block.is_error ? "✗" : "✓";
    row.el.classList.toggle("error", !!block.is_error);
    const text = Array.isArray(block.content)
      ? block.content.map((c: { text?: string }) => c.text ?? "").join("\n")
      : String(block.content ?? "");
    row.detail.textContent += `\n\n→ ${text.slice(0, 4000)}`;
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onResult(ev: any) {
    const parts = [ev.is_error ? "Stopped with an error" : "Done"];
    if (typeof ev.total_cost_usd === "number") parts.push(`$${ev.total_cost_usd.toFixed(4)}`);
    if (ev.num_turns) parts.push(`${ev.num_turns} turns`);
    if (ev.duration_ms) parts.push(`${(ev.duration_ms / 1000).toFixed(1)}s`);
    this.note(parts.join(" · "));
    if (ev.is_error && typeof ev.result === "string") {
      if (/log ?in|auth/i.test(ev.result)) this.loginNeeded(ev.result);
      else this.error(ev.result);
    }
    const denied = ev.permission_denials ?? [];
    if (denied.length) {
      const names = [...new Set(denied.map((d: { tool_name?: string }) => d.tool_name ?? "a tool"))].join(", ");
      this.note(`Claude wanted to use ${names}, which isn't allowed in “Paper tools” mode. Switch to “Paper tools + file edits” to allow it.`);
    }
  }

  private loginNeeded(detail: string) {
    this.log.append(
      h("div", { class: "msg error" },
        h("p", {}, "Claude Code needs you to sign in."),
        h("pre", {}, detail.slice(-400)),
        h("button", { class: "primary small", onclick: () => void this.openTerminal("login") }, "Sign in…"),
      ),
    );
    this.scroll();
  }

  private note(text: string) {
    this.log.append(h("div", { class: "chat-note muted" }, text));
    this.scroll();
  }

  private error(text: string) {
    this.log.append(h("div", { class: "msg error" }, h("pre", {}, text)));
    this.scroll();
  }

  private scroll() {
    this.log.scrollTop = this.log.scrollHeight;
  }

  get title() {
    return this.paperTitle;
  }
}
