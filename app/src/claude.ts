// The Claude panel: chat with Claude Code about the paper. Claude edits
// through the paper's MCP tools, so its changes stream into the editor and
// history like a co-author's.
import { api, events, type ChatAttachment, type ClaudeAuthInfo, type ClaudePermissionReply, type ClaudeSession, type EditorContext } from "./api";
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

type ProviderId = "claudeai" | "console" | "bedrock" | "vertex" | "foundry";
type AssistantId = "claude" | "chatgpt" | "gemini";

interface ModelOption {
  id: string;
  label: string;
}

interface SignInChoice {
  assistant: AssistantId;
  provider: string;
  model: string;
  foundryResource: string;
  foundryApiKey: string;
  apiKey: string;
}

interface AccountOption {
  id: string;
  label: string;
  hint: string;
}

const AUTH_KEY = "letswritepaper.claudeAuth";
const CUSTOM_MODEL = "__custom__";

const ASSISTANTS: { id: AssistantId; label: string; product: string; placeholder: string }[] = [
  { id: "claude", label: "Claude", product: "Claude Code", placeholder: "Ask Claude… Paste an image or file, or type / for usage, status, and plugins" },
  { id: "chatgpt", label: "ChatGPT", product: "Codex", placeholder: "Ask ChatGPT… Paste an image or file (Enter to send, Shift+Enter for a new line)" },
  { id: "gemini", label: "Gemini", product: "Antigravity", placeholder: "Ask Gemini… Paste a file and Gemini can read it (Enter to send, Shift+Enter for a new line)" },
];

interface SlashCommand {
  name: string;
  detail: string;
  /** A complete command can be sent as soon as it is chosen. */
  send: boolean;
}

const SLASH: SlashCommand[] = [
  { name: "/usage", detail: "Session, weekly, and extra-usage limits for this account", send: true },
  { name: "/status", detail: "Claude Code version, account, and plan limits", send: true },
  { name: "/cost", detail: "Same account limits as /usage", send: true },
  { name: "/stats", detail: "Same account limits as /usage", send: true },
  { name: "/plugin list", detail: "Plugins and extensions. Opens in Claude Code's terminal", send: true },
  { name: "/plugin", detail: "Manage plugins. Opens in the terminal", send: false },
  { name: "/reload-plugins", detail: "Reload plugins. Opens in the terminal", send: true },
  { name: "/skills", detail: "Skills Claude Code can use. Opens in the terminal", send: true },
  { name: "/reload-skills", detail: "Rescan skills. Opens in the terminal", send: true },
  { name: "/doctor", detail: "Check the install. Opens in the terminal", send: true },
  { name: "/help", detail: "List commands. Opens in the terminal", send: true },
  { name: "/context", detail: "How full this chat's context window is", send: true },
  { name: "/compact", detail: "Summarize the chat to free context. Add a focus after it", send: false },
  { name: "/memory", detail: "What Claude remembers. Opens in the terminal", send: true },
  { name: "/permissions", detail: "What Claude is allowed to do. Opens in the terminal", send: true },
  { name: "/mcp", detail: "Connected MCP servers. Opens in the terminal", send: true },
  { name: "/diff", detail: "Uncommitted changes. Opens in the terminal", send: true },
  { name: "/mcp__paper__write-section", detail: "Paper skill: write a section", send: false },
  { name: "/mcp__paper__review-paper", detail: "Paper skill: review the paper", send: false },
  { name: "/mcp__paper__address-reviews", detail: "Paper skill: address review comments", send: false },
];

const MAX_FILE_BYTES = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES = 8 * 1024 * 1024;
const MAX_FILES = 8;

interface PendingFile {
  name: string;
  media_type: string;
  data: string;
  bytes: number;
  preview?: string;
}

function slashToken(value: string): string {
  const trimmed = value.trim();
  if (!trimmed.startsWith("/") || trimmed.includes("\n")) return "";
  return trimmed.split(/\s/, 1)[0] ?? "";
}

function isSlashMessage(text: string): boolean {
  const line = text.trim().split("\n", 1)[0]?.trim() ?? "";
  return /^\/[A-Za-z]/.test(line.split(/\s/, 1)[0] ?? "");
}

function mediaFromName(name: string): string {
  switch (name.split(".").pop()?.toLowerCase()) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "pdf":
      return "application/pdf";
    case "json":
      return "application/json";
    case "svg":
      return "image/svg+xml";
    default:
      return "application/octet-stream";
  }
}

function systemNote(ev: { message?: unknown; result?: unknown; content?: unknown; text?: unknown }): string {
  for (const value of [ev.message, ev.result, ev.content, ev.text]) {
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result ?? "");
      const comma = url.indexOf(",");
      resolve(comma >= 0 ? url.slice(comma + 1) : "");
    };
    reader.onerror = () => reject(reader.error ?? new Error(`Could not read ${file.name}`));
    reader.readAsDataURL(file);
  });
}

const CHATGPT_ACCOUNTS: AccountOption[] = [
  { id: "chatgpt", label: "ChatGPT account", hint: "Opens a terminal to sign in with ChatGPT. Usage comes from that plan." },
  { id: "apikey", label: "OpenAI API key", hint: "Usage is billed to your OpenAI Platform account. The key is given to Codex and is not saved in the paper. Leave it empty to keep the current Codex login." },
];

const GEMINI_ACCOUNTS: AccountOption[] = [
  { id: "google", label: "Google AI Pro or Ultra", hint: "Opens Antigravity so you can sign in with the Google account that has Pro or Ultra. A free Google account can sign in too, with a smaller quota." },
  { id: "apikey", label: "Gemini API key", hint: "Uses a key from Google AI Studio. The key lasts until you quit the app and is not saved in the paper. Antigravity keeps using the API until you choose the Google account again." },
];

const CHATGPT_MODELS: ModelOption[] = [
  { id: "", label: "Account default" },
  { id: "gpt-6.1-sol", label: "GPT-6.1 Sol" },
];

const GEMINI_MODELS: ModelOption[] = [
  { id: "", label: "Account default" },
  { id: "gemini-4-argon-high", label: "Gemini 4 Argon" },
  { id: "gemini-3.8-flash-high", label: "Gemini 3.8 Flash" },
  { id: "gemini-3.1-pro-high", label: "Gemini 3.1 Pro" },
];

function assistantMeta(id: AssistantId) {
  return ASSISTANTS.find((item) => item.id === id) ?? ASSISTANTS[0];
}

function isAssistant(value: string | undefined): value is AssistantId {
  return ASSISTANTS.some((item) => item.id === value);
}

const PROVIDERS: { id: ProviderId; label: string; hint: string }[] = [
  { id: "claudeai", label: "Claude account", hint: "Opens a terminal to sign in with a claude.ai account (Pro, Max, Team, or Enterprise)." },
  { id: "console", label: "Anthropic Console", hint: "Opens a terminal to sign in with an Anthropic Console account. Usage is billed as API usage." },
  { id: "bedrock", label: "Amazon Bedrock", hint: "Opens Claude Code. At the login prompt, choose 3rd-party platform, then Amazon Bedrock. The wizard lists the models your account can use." },
  { id: "vertex", label: "Google Cloud Agent Platform", hint: "Opens Claude Code. At the login prompt, choose 3rd-party platform, then Google Vertex AI. The wizard lists the models your project can use." },
  { id: "foundry", label: "Microsoft Foundry", hint: "The Claude panel uses this Azure resource. Leave the API key empty to sign in with az login in a terminal. A key you enter is kept until you quit the app." },
];

// Alias targets differ by provider. See Claude Code's model configuration:
// Anthropic serves the newest alias, Bedrock and Agent Platform lag on Sonnet and Haiku,
// and Foundry's Opus alias is Opus 4.6.
const ANTHROPIC_MODELS: ModelOption[] = [
  { id: "", label: "Account default" },
  { id: "fable", label: "Fable 5.1" },
  { id: "opus", label: "Opus 5.5" },
  { id: "sonnet", label: "Sonnet 5.5" },
  { id: "haiku", label: "Haiku 5.5" },
  { id: "opusplan", label: "Opus 5.5 to plan, Sonnet 5.5 to edit" },
];

const MODELS: Record<ProviderId, ModelOption[]> = {
  claudeai: ANTHROPIC_MODELS,
  console: [
    { id: "", label: "Console default" },
    { id: "fable", label: "Fable 5.1" },
    { id: "opus", label: "Opus 5.5" },
    { id: "sonnet", label: "Sonnet 5.5" },
    { id: "haiku", label: "Haiku 5.5" },
    { id: "opusplan", label: "Opus 5.5 to plan, Sonnet 5.5 to edit" },
  ],
  bedrock: [
    { id: "", label: "Provider default" },
    { id: "fable", label: "Fable 5.1" },
    { id: "opus", label: "Opus 5.5" },
    { id: "sonnet", label: "Sonnet 4.5" },
    { id: "haiku", label: "Haiku 4.5" },
    { id: "opusplan", label: "Opus 5.5 to plan, Sonnet 4.5 to edit" },
  ],
  vertex: [
    { id: "", label: "Provider default" },
    { id: "fable", label: "Fable 5.1" },
    { id: "opus", label: "Opus 5.5" },
    { id: "sonnet", label: "Sonnet 4.5" },
    { id: "haiku", label: "Haiku 4.5" },
    { id: "opusplan", label: "Opus 5.5 to plan, Sonnet 4.5 to edit" },
  ],
  foundry: [
    { id: "", label: "Provider default (Opus 4.6)" },
    { id: "fable", label: "Fable 5.1" },
    { id: "opus", label: "Opus 4.6" },
    { id: "sonnet", label: "Sonnet 4.5" },
    { id: "haiku", label: "Haiku 4.5" },
    { id: "opusplan", label: "Opus 4.6 to plan, Sonnet 4.5 to edit" },
  ],
};

function isProvider(value: string | undefined): value is ProviderId {
  return PROVIDERS.some((p) => p.id === value);
}

function accountsFor(assistant: AssistantId): AccountOption[] {
  if (assistant === "chatgpt") return CHATGPT_ACCOUNTS;
  if (assistant === "gemini") return GEMINI_ACCOUNTS;
  return PROVIDERS;
}

function defaultProvider(assistant: AssistantId): string {
  return accountsFor(assistant)[0]?.id ?? "claudeai";
}

function modelsFor(assistant: AssistantId, provider: string): ModelOption[] {
  if (assistant === "chatgpt") return CHATGPT_MODELS;
  if (assistant === "gemini") return GEMINI_MODELS;
  return MODELS[isProvider(provider) ? provider : "claudeai"];
}

interface StoredAuth {
  assistant: AssistantId;
  provider: string;
  model: string;
  foundryResource: string;
  stored: boolean;
}

function loadAuth(): StoredAuth {
  const fallback: StoredAuth = { assistant: "claude", provider: "claudeai", model: "", foundryResource: "", stored: false };
  try {
    const raw = localStorage.getItem(AUTH_KEY);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as Partial<StoredAuth>;
    const assistant = isAssistant(parsed.assistant) ? parsed.assistant : "claude";
    return {
      assistant,
      provider: accountsFor(assistant).some((item) => item.id === parsed.provider) ? parsed.provider! : defaultProvider(assistant),
      model: typeof parsed.model === "string" ? parsed.model : "",
      foundryResource: typeof parsed.foundryResource === "string" ? parsed.foundryResource : "",
      stored: true,
    };
  } catch {
    return fallback;
  }
}

function saveAuth(auth: Pick<StoredAuth, "assistant" | "provider" | "model" | "foundryResource">) {
  localStorage.setItem(AUTH_KEY, JSON.stringify(auth));
}

/** Sign-in dialog: assistant, then the account and models that assistant offers. */
function chooseSignIn(initial: SignInChoice, status: ClaudeAuthInfo | null): Promise<SignInChoice | null> {
  return new Promise((resolve) => {
    const assistant = h("select", { "aria-label": "Assistant" });
    for (const item of ASSISTANTS) assistant.append(h("option", { value: item.id }, item.label));
    assistant.value = initial.assistant;

    const provider = h("select", { "aria-label": "Account or provider" });
    const model = h("select", { "aria-label": "Model" });
    const custom = h("input", { type: "text", placeholder: "Model id" });
    const customLabel = h("label", {}, h("span", {}, "Model id"), custom);
    const resource = h("input", { type: "text", placeholder: "my-resource", value: initial.foundryResource });
    const foundryKey = h("input", { type: "password", autocomplete: "off", placeholder: "Leave empty to use az login", value: initial.foundryApiKey });
    const foundry = h(
      "div",
      { class: "form" },
      h("label", {}, h("span", {}, "Foundry resource name"), resource),
      h("label", {}, h("span", {}, "API key"), foundryKey),
    );
    const accountKey = h("input", { type: "password", autocomplete: "off", placeholder: "Paste the key", value: initial.apiKey });
    const accountKeyLabel = h("label", {}, h("span", {}, "API key"), accountKey);
    const hint = h("p", { class: "muted" }, "");

    const selectedAssistant = () => (isAssistant(assistant.value) ? assistant.value : "claude");

    const fillAccounts = (keepProvider: boolean) => {
      const id = selectedAssistant();
      const accounts = accountsFor(id);
      const previous = provider.value;
      clear(provider, ...accounts.map((item) => h("option", { value: item.id }, item.label)));
      const wanted = keepProvider ? previous || initial.provider : defaultProvider(id);
      provider.value = accounts.some((item) => item.id === wanted) ? wanted : defaultProvider(id);
    };

    const fillModels = (selected: string) => {
      const id = selectedAssistant();
      const account = provider.value || defaultProvider(id);
      const options = modelsFor(id, account);
      const customSelected = selected !== "" && !options.some((option) => option.id === selected);
      clear(model, ...options.map((option) => h("option", { value: option.id }, option.label)), h("option", { value: CUSTOM_MODEL }, "Other model id…"));
      if (customSelected) {
        model.value = CUSTOM_MODEL;
        custom.value = selected;
      } else {
        model.value = options.some((option) => option.id === selected) ? selected : "";
        if (!customSelected) custom.value = "";
      }
      customLabel.hidden = model.value !== CUSTOM_MODEL;
      custom.placeholder = id === "claude" ? "Model id, Bedrock inference profile, or Foundry deployment name" : "Model id";
      const accountHint = accountsFor(id).find((item) => item.id === account)?.hint ?? "";
      hint.textContent = accountHint;
      foundry.hidden = !(id === "claude" && account === "foundry");
      accountKeyLabel.hidden = account !== "apikey";
    };
    assistant.addEventListener("change", () => {
      fillAccounts(false);
      fillModels("");
    });
    provider.addEventListener("change", () => fillModels(model.value === CUSTOM_MODEL ? custom.value.trim() : model.value));
    model.addEventListener("change", (event) => {
      customLabel.hidden = model.value !== CUSTOM_MODEL;
      if (model.value === CUSTOM_MODEL && event.isTrusted) custom.focus();
    });
    fillAccounts(true);
    fillModels(initial.model);

    const close = (value: SignInChoice | null) => {
      overlay.remove();
      resolve(value);
    };
    const submit = () => {
      const id = selectedAssistant();
      const account = provider.value || defaultProvider(id);
      const modelId = model.value === CUSTOM_MODEL ? custom.value.trim() : model.value;
      if (model.value === CUSTOM_MODEL && !modelId) {
        custom.focus();
        custom.classList.add("invalid");
        return;
      }
      if (id === "claude" && account === "foundry" && !resource.value.trim()) {
        resource.focus();
        resource.classList.add("invalid");
        return;
      }
      if (id === "gemini" && account === "apikey" && !accountKey.value.trim()) {
        accountKey.focus();
        accountKey.classList.add("invalid");
        return;
      }
      close({
        assistant: id,
        provider: account,
        model: modelId,
        foundryResource: resource.value.trim(),
        foundryApiKey: foundryKey.value.trim(),
        apiKey: accountKey.value.trim(),
      });
    };
    const current = status?.logged_in
      ? h("p", {}, `Current sign-in: ${status.detail || accountsFor(selectedAssistant()).find((item) => item.id === status.provider)?.label || status.provider}`)
      : null;
    const overlay = h(
      "div",
      { class: "overlay", onmousedown: (e: Event) => e.target === overlay && close(null) },
      h(
        "div",
        { class: "modal", role: "dialog", "aria-label": "Sign in" },
        h("h2", {}, "Sign in"),
        h("div", { class: "form" },
          current,
          h("p", { class: "muted" }, "Choose Claude, ChatGPT, or Gemini, then the account and model that should answer. You can switch later; each assistant keeps its own chats."),
          h("label", {}, h("span", {}, "Assistant"), assistant),
          h("label", {}, h("span", {}, "Account or provider"), provider),
          h("label", {}, h("span", {}, "Model"), model),
          customLabel,
          foundry,
          accountKeyLabel,
          hint,
        ),
        h("div", { class: "modal-actions" },
          h("button", { class: "ghost", onclick: () => close(null) }, "Cancel"),
          h("button", { class: "primary", onclick: submit }, "Continue"),
        ),
      ),
    );
    overlay.addEventListener("keydown", (e) => {
      if (e.key === "Escape") close(null);
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
    });
    document.body.append(overlay);
    assistant.focus();
  });
}

const QUICK_ACTIONS: [string, string][] = [
  ["Improve selection", "Improve the selected text for clarity and concision. Keep the meaning, citations, labels and math. Apply it with edit_document."],
  ["Review this section", "Read the section I'm in with section_context and give me the three most important problems, as review comments (start_review, add_review_comment, finish_review). Don't edit."],
  ["Address open comments", "Work through the open review comments on this paper: fix what you can with edit_document (or propose_edit for big changes) and record a response for each with respond_to_comment."],
  ["Mock peer review", "Run a mock peer review of the whole paper as the methods-skeptic persona, following the peer-review skill."],
  ["Check it compiles", "Compile the paper and fix any errors you find."],
];

type ToolRow = {
  el: HTMLElement;
  status: HTMLElement;
  detail: HTMLElement;
};

function toolDetail(input: unknown): string {
  if (!input || typeof input !== "object") return "";
  const obj = input as Record<string, unknown>;
  for (const key of ["query", "url", "command", "file_path", "path", "pattern", "prompt", "description", "plan"]) {
    const value = obj[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  const text = JSON.stringify(obj, null, 2);
  return text === "{}" ? "" : text;
}

function suggestionLabel(suggestion: { rules?: { toolName?: string; ruleContent?: string }[]; destination?: string }): string | null {
  const rules = Array.isArray(suggestion.rules) ? suggestion.rules : [];
  const what = rules
    .map((rule) => (rule.ruleContent ? `${rule.toolName}(${rule.ruleContent})` : rule.toolName))
    .filter((rule): rule is string => !!rule);
  if (!what.length) return null;
  const scope: Record<string, string> = {
    session: "for this chat",
    localSettings: "in this project",
    projectSettings: "for this project",
    userSettings: "for every project",
    cliArg: "for this run",
  };
  return `Always allow ${what.join(", ")} ${scope[suggestion.destination ?? ""] ?? "from now on"}`;
}

export class ClaudePanel {
  private chatId = crypto.randomUUID();
  private sessionId: string | null = null;
  private running = false;
  private busy = false;
  private mode: "paper" | "edits" = "paper";
  private assistant: AssistantId = "claude";
  private provider = "claudeai";
  private model = "";
  private foundryResource = "";
  /** Kept for this app session only; it is not written to disk. */
  private foundryApiKey = "";
  private apiKey = "";
  private heading: HTMLElement | null = null;
  private remoteBtn: HTMLButtonElement | null = null;
  private fillingModel = false;
  private log: HTMLElement;
  private input: HTMLTextAreaElement;
  private sendBtn: HTMLButtonElement;
  private statusLine: HTMLElement;
  private sessionSelect: HTMLSelectElement;
  private modelSelect: HTMLSelectElement;
  private shareContext = true;
  private bubbles = new Map<string, { el: HTMLElement; text: string; streamed: boolean }>();
  private tools = new Map<string, ToolRow>();
  private prompts = new Map<string, HTMLElement>();
  private promptedThisTurn = false;
  private currentMsg: string | null = null;
  private renderQueued = new Set<string>();
  private stderr: string[] = [];
  private ready = false;
  private pending: PendingFile[] = [];
  private attachRow: HTMLElement;
  private slashMenu: HTMLElement;
  private slashMatches: SlashCommand[] = [];
  private slashIndex = 0;
  private slashTokenShown = "";
  private fileInput: HTMLInputElement;
  private streamedThisTurn = false;

  constructor(
    private root: HTMLElement,
    private context: () => EditorContext,
    private paperTitle: string,
  ) {
    this.log = h("div", { class: "chat-log", "aria-live": "polite" });
    this.input = h("textarea", { class: "chat-input", rows: 3, placeholder: assistantMeta("claude").placeholder });
    this.attachRow = h("div", { class: "attach-row" });
    this.attachRow.hidden = true;
    this.slashMenu = h("div", { class: "slash-menu", role: "listbox", "aria-label": "Claude commands" });
    this.slashMenu.hidden = true;
    this.fileInput = h("input", { type: "file", multiple: true });
    this.fileInput.hidden = true;
    this.fileInput.addEventListener("change", () => {
      const files = [...(this.fileInput.files ?? [])];
      this.fileInput.value = "";
      if (files.length) void this.addFiles(files);
    });
    this.sendBtn = h("button", { class: "primary", onclick: () => void this.submit() }, "Send");
    this.statusLine = h("span", { class: "muted small" }, "");
    this.sessionSelect = h("select", { class: "small-select", "aria-label": "Chat session", onchange: () => void this.switchSession() });
    const saved = loadAuth();
    this.assistant = saved.assistant;
    this.provider = saved.provider;
    this.model = saved.model;
    this.foundryResource = saved.foundryResource;
    this.input.placeholder = assistantMeta(this.assistant).placeholder;
    this.modelSelect = h("select", {
      class: "small-select",
      "aria-label": "Model",
      title: "Model this provider uses",
      onchange: () => {
        if (this.fillingModel) return;
        this.model = this.modelSelect.value;
        this.persist();
        if (this.running) void this.restart();
      },
    });
    this.fillModelSelect();
    this.input.addEventListener("keydown", (e) => {
      if (this.slashMatches.length && !this.slashMenu.hidden) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          this.slashIndex = Math.min(this.slashIndex + 1, this.slashMatches.length - 1);
          this.paintSlash();
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          this.slashIndex = Math.max(this.slashIndex - 1, 0);
          this.paintSlash();
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          this.closeSlash();
          return;
        }
        if (e.key === "Tab") {
          e.preventDefault();
          const cmd = this.slashMatches[this.slashIndex];
          if (cmd) this.applySlash(cmd);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
          const token = slashToken(this.input.value);
          const cmd = this.slashMatches[this.slashIndex];
          if (cmd && this.input.value.trim() === token) {
            e.preventDefault();
            if (cmd.send) void this.submit(cmd.name);
            else this.applySlash(cmd);
            return;
          }
        }
      }
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        void this.submit();
      }
    });
    this.input.addEventListener("input", () => this.refreshSlash());
    this.input.addEventListener("paste", (e) => {
      const data = e.clipboardData;
      if (!data) return;
      const files = [...data.files];
      if (!files.length) return;
      e.preventDefault();
      const pasted = data.getData("text/plain");
      if (pasted) {
        const start = this.input.selectionStart ?? this.input.value.length;
        const end = this.input.selectionEnd ?? start;
        this.input.value = this.input.value.slice(0, start) + pasted + this.input.value.slice(end);
        this.refreshSlash();
      }
      void this.addFiles(files);
    });
    void events.onClaude(({ chat_id, event }) => {
      if (chat_id === this.chatId) this.onEvent(event);
    });
  }

  /** Called when the panel becomes visible. */
  async show() {
    if (this.ready) return;
    const status = await api.claudeStatus(this.assistant).catch(() => null);
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
        out.textContent = "";
        const unlisten = await events.onInstallLog((line) => {
          out.textContent += line + "\n";
          out.scrollTop = out.scrollHeight;
        });
        try {
          const s = await api.claudeInstall(this.assistant);
          if (s.installed) {
            toast(`${assistantMeta(this.assistant).product} ${s.version ?? ""} installed`);
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
    }, `Install ${assistantMeta(this.assistant).product}`);
    const meta = assistantMeta(this.assistant);
    clear(
      this.root,
      h(
        "div",
        { class: "claude-setup" },
        h("h3", {}, `${meta.label} in your paper`),
        h("p", {}, `This panel runs ${meta.product} on this computer, with your account. It can draft, edit, review and compile through the paper's tools, and every change shows up live with its name in the history.`),
        h("p", { class: "muted" }, this.assistant === "gemini"
          ? `${meta.product} isn't installed yet. The app downloads the current release and checks its published checksum before installing.`
          : `${meta.product} isn't installed yet. The official installer runs:`),
        this.assistant === "gemini" ? null : h("code", { class: "block" }, command),
        h("div", { class: "actions" }, btn, h("button", { onclick: () => ((this.ready = false), void this.show()) }, "I installed it"), h("button", { onclick: () => void this.signIn() }, "Use a different assistant")),
        out,
        h("p", { class: "muted small" }, "You can use Claude, ChatGPT, or Gemini. Each one signs in with its own account."),
      ),
    );
  }

  private renderChat(version: string) {
    const meta = assistantMeta(this.assistant);
    const modeSelect = h("select", {
      class: "small-select",
      "aria-label": `What ${meta.label} may do`,
      title: this.assistant === "claude"
        ? "Paper tools: Claude edits through the paper's tools. Web search and other tools ask you to allow them here. File edits: Claude may also edit files without asking."
        : "Paper tools: the assistant is asked to edit through the paper's tools. File edits: it may also change files directly.",
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
    this.heading = h("strong", {}, meta.label);
    this.remoteBtn = h("button", { class: "icon", title: "Continue on your phone or another computer", "aria-label": "Continue on another device", onclick: () => this.continueElsewhere() }, "📱");
    this.remoteBtn.hidden = this.assistant !== "claude";
    clear(
      this.root,
      h(
        "div",
        { class: "pane-header" },
        this.heading,
        this.sessionSelect,
        h("span", { class: "spacer" }),
        this.remoteBtn,
        h("button", { class: "icon", title: "Open this chat in a terminal", "aria-label": "Open in terminal", onclick: () => void this.openTerminal("resume") }, "⌨"),
        h("button", { class: "icon", title: "Choose an assistant, account, and model", "aria-label": "Sign in", onclick: () => void this.signIn() }, "🔑"),
      ),
      this.log,
      h(
        "div",
        {
          class: "chat-compose",
          ondragover: (e: Event) => {
            const drag = e as DragEvent;
            if (!drag.dataTransfer?.types.includes("Files")) return;
            e.preventDefault();
            (e.currentTarget as HTMLElement).classList.add("drop");
          },
          ondragleave: (e: Event) => (e.currentTarget as HTMLElement).classList.remove("drop"),
          ondrop: (e: Event) => {
            const drag = e as DragEvent;
            e.preventDefault();
            (e.currentTarget as HTMLElement).classList.remove("drop");
            const files = [...(drag.dataTransfer?.files ?? [])];
            if (files.length) void this.addFiles(files);
          },
        },
        h("div", { class: "chips" }, ...QUICK_ACTIONS.map(([label, prompt]) => h("button", { class: "chip add", onclick: () => void this.submit(prompt) }, label))),
        this.slashMenu,
        this.attachRow,
        this.input,
        this.fileInput,
        h(
          "div",
          { class: "chat-bar" },
          this.modelSelect,
          modeSelect,
          ctxToggle,
          this.statusLine,
          h("span", { class: "spacer" }),
          h("button", { class: "icon", title: "Attach images or files", "aria-label": "Attach images or files", onclick: () => this.fileInput.click() }, "📎"),
          this.sendBtn,
        ),
      ),
    );
    this.statusLine.textContent = version ? `${meta.product} ${version.split(" ")[0]}` : "";
    this.input.placeholder = meta.placeholder;
    this.paintAttachments();
    this.refreshSlash();
    if (!this.log.childElementCount) {
      const credit = this.assistant === "claude" ? "claude-code" : this.assistant === "chatgpt" ? "codex" : "gemini";
      const extra = this.assistant === "claude"
        ? " Paste or drop images and files, or type / for usage, status, and plugins."
        : " Paste or drop images and files.";
      this.log.append(
        h("div", { class: "chat-empty muted" },
          h("p", {}, `Ask ${meta.label} to work on your paper. It sees which file and section you're in and what you've selected.${extra}`),
          h("p", {}, `Its edits appear live in the editor, credited to ${credit} in History and Blame.`),
        ),
      );
    }
  }

  private async loadSessions(selected?: string) {
    const sessions = (await api.claudeSessions().catch(() => [] as ClaudeSession[])).filter(
      (s) => (s.assistant || "claude") === this.assistant,
    );
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
    this.prompts.clear();
    clear(this.log, h("div", { class: "chat-note muted" }, id ? "Resuming an earlier chat — Claude remembers it; earlier messages aren't shown here." : "New chat."));
  }

  private async restart() {
    await this.stop();
    await this.ensureRunning();
  }

  private async ensureRunning() {
    if (this.running) return;
    await api.claudeStart(
      this.chatId,
      this.mode,
      this.sessionId ?? undefined,
      this.model || undefined,
      this.provider,
      this.foundryResource || undefined,
      this.foundryApiKey || undefined,
      this.assistant,
      this.assistant === "gemini" ? this.apiKey || undefined : undefined,
    );
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

  private async addFiles(files: File[]) {
    let total = this.pending.reduce((sum, file) => sum + file.bytes, 0);
    for (const file of files) {
      if (this.pending.length >= MAX_FILES) {
        toast("You can attach up to 8 files.", "error");
        break;
      }
      if (file.size > MAX_FILE_BYTES) {
        toast(`${file.name || "That file"} is larger than 4 MB.`, "error");
        continue;
      }
      if (total + file.size > MAX_TOTAL_BYTES) {
        toast("Attachments are limited to 8 MB altogether.", "error");
        break;
      }
      try {
        const data = await readBase64(file);
        if (!data) {
          toast(`${file.name || "That file"} is empty.`, "error");
          continue;
        }
        const media = file.type || mediaFromName(file.name);
        const preview = media.startsWith("image/") && media !== "image/svg+xml" ? URL.createObjectURL(file) : undefined;
        this.pending.push({ name: file.name || "file", media_type: media, data, bytes: file.size, preview });
        total += file.size;
      } catch (e) {
        toast(String(e), "error");
      }
    }
    this.paintAttachments();
  }

  private paintAttachments() {
    clear(
      this.attachRow,
      ...this.pending.map((file, index) =>
        h(
          "span",
          { class: "attach-chip" },
          file.preview ? h("img", { src: file.preview, alt: "" }) : "",
          h("span", {}, file.name),
          h("button", {
            class: "icon",
            title: "Remove",
            "aria-label": `Remove ${file.name}`,
            onclick: () => this.removeAttachment(index),
          }, "×"),
        ),
      ),
    );
    this.attachRow.hidden = this.pending.length === 0;
  }

  private removeAttachment(index: number) {
    const [file] = this.pending.splice(index, 1);
    if (file?.preview) URL.revokeObjectURL(file.preview);
    this.paintAttachments();
  }

  private clearPending() {
    this.pending = [];
    this.paintAttachments();
  }

  private userBubble(caption: string, files: PendingFile[]) {
    const bubble = h("div", { class: "msg user" }, h("div", {}, caption));
    if (!files.length) return bubble;
    const row = h("div", { class: "attach-sent" });
    for (const file of files) {
      if (file.preview) row.append(h("img", { class: "attach-thumb", src: file.preview, alt: file.name }));
      else row.append(h("span", { class: "attach-chip" }, file.name));
    }
    bubble.append(row);
    return bubble;
  }

  private refreshSlash() {
    if (this.assistant !== "claude") {
      this.closeSlash();
      return;
    }
    const token = slashToken(this.input.value).toLowerCase();
    if (!token) {
      this.closeSlash();
      return;
    }
    this.slashMatches = SLASH.filter((cmd) => cmd.name.startsWith(token));
    if (!this.slashMatches.length) {
      this.closeSlash();
      return;
    }
    if (token !== this.slashTokenShown) {
      this.slashTokenShown = token;
      this.slashIndex = 0;
    }
    this.slashIndex = Math.min(this.slashIndex, this.slashMatches.length - 1);
    this.paintSlash();
    this.slashMenu.hidden = false;
  }

  private paintSlash() {
    clear(
      this.slashMenu,
      ...this.slashMatches.map((cmd, index) =>
        h(
          "button",
          {
            type: "button",
            class: `slash-item${index === this.slashIndex ? " active" : ""}`,
            role: "option",
            onclick: () => {
              const typed = this.input.value.trim().slice(slashToken(this.input.value).length).trim();
              if (cmd.send && !typed) void this.submit(cmd.name);
              else this.applySlash(cmd);
            },
          },
          h("code", {}, cmd.name),
          h("span", {}, cmd.detail),
        ),
      ),
    );
  }

  private applySlash(cmd: SlashCommand) {
    const current = this.input.value.trim();
    const token = slashToken(current);
    const rest = token ? current.slice(token.length).trim() : "";
    this.input.value = rest ? `${cmd.name} ${rest}` : cmd.send ? cmd.name : `${cmd.name} `;
    this.closeSlash();
    this.input.focus();
  }

  private closeSlash() {
    this.slashMatches = [];
    this.slashIndex = 0;
    this.slashTokenShown = "";
    this.slashMenu.hidden = true;
    clear(this.slashMenu);
  }

  async submit(text?: string) {
    const fromComposer = text === undefined;
    const msg = (text ?? this.input.value).trim();
    const files = fromComposer ? this.pending.slice() : [];
    if ((!msg && files.length === 0) || this.busy) return;
    if (!this.ready) await this.show();
    const slash = this.assistant === "claude" && isSlashMessage(msg);
    const sending = slash ? [] : files;
    const caption = msg || "Please look at the attached files.";
    if (fromComposer) this.input.value = "";
    this.closeSlash();
    this.log.querySelector(".chat-empty")?.remove();
    this.log.append(this.userBubble(caption, sending));
    this.scroll();
    this.stderr = [];
    this.streamedThisTurn = false;
    this.setBusy(true);
    try {
      await this.ensureRunning();
      const attachments: ChatAttachment[] = sending.map((file) => ({
        name: file.name,
        media_type: file.media_type,
        data: file.data,
      }));
      await api.claudeSend(
        this.chatId,
        caption,
        slash || !this.shareContext ? undefined : this.context(),
        attachments,
      );
      if (fromComposer && !slash) this.clearPending();
      if (fromComposer && slash && files.length) {
        toast("Sent the command. The attached files stay here for the next message.");
      }
    } catch (e) {
      if (fromComposer && !this.input.value) this.input.value = msg;
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

  private fillModelSelect() {
    const options = modelsFor(this.assistant, this.provider);
    const known = options.some((option) => option.id === this.model);
    this.fillingModel = true;
    clear(
      this.modelSelect,
      ...options.map((option) => h("option", { value: option.id }, option.label)),
      ...(known || !this.model ? [] : [h("option", { value: this.model }, this.model)]),
    );
    this.modelSelect.value = this.model;
    this.fillingModel = false;
  }

  private persist() {
    saveAuth({ assistant: this.assistant, provider: this.provider, model: this.model, foundryResource: this.foundryResource });
  }

  private async signIn() {
    const saved = loadAuth();
    const asking = saved.stored ? saved.assistant : this.assistant;
    const status = await api.claudeAuth(asking).catch(() => null);
    const provider = saved.stored ? saved.provider : status?.provider && accountsFor(asking).some((item) => item.id === status.provider) ? status.provider : saved.provider;
    const choice = await chooseSignIn(
      {
        assistant: asking,
        provider,
        model: saved.stored ? saved.model : this.model,
        foundryResource: this.foundryResource,
        foundryApiKey: this.foundryApiKey,
        apiKey: this.apiKey,
      },
      status,
    );
    if (!choice) return;
    const switched = choice.assistant !== this.assistant;
    this.assistant = choice.assistant;
    this.provider = choice.provider;
    this.model = choice.model;
    this.foundryResource = choice.foundryResource;
    this.foundryApiKey = choice.foundryApiKey;
    this.apiKey = choice.apiKey;
    this.persist();
    this.fillModelSelect();
    if (this.heading) this.heading.textContent = assistantMeta(this.assistant).label;
    if (this.remoteBtn) this.remoteBtn.hidden = this.assistant !== "claude";
    this.input.placeholder = assistantMeta(this.assistant).placeholder;
    this.closeSlash();
    if (this.running) {
      await this.stop();
      if (!switched && this.log.isConnected) this.note("Account or model changed. Send a message to continue with it.");
    }
    if (switched) {
      this.sessionId = null;
      this.chatId = crypto.randomUUID();
      this.bubbles.clear();
      this.tools.clear();
      this.prompts.clear();
      if (this.log.isConnected) {
        clear(this.log, h("div", { class: "chat-note muted" }, `Switched to ${assistantMeta(this.assistant).label}. Earlier chats for this assistant are in the menu.`));
        await this.loadSessions();
      }
    }
    const installed = await api.claudeStatus(this.assistant).catch(() => null);
    if (!installed?.installed) {
      this.ready = false;
      this.renderInstall(installed?.install_command ?? "");
      return;
    }
    if (!this.ready) {
      this.ready = true;
      this.renderChat(installed.version ?? "");
      await this.loadSessions();
    }
    try {
      if (choice.assistant === "chatgpt" && choice.provider === "apikey") {
        if (choice.apiKey) await api.assistantStoreKey("chatgpt", choice.apiKey);
        toast(choice.apiKey ? "Codex is signed in with that API key. Send a message to start." : "Using the current Codex login. Send a message to start.");
        return;
      }
      if (choice.assistant === "gemini" && choice.provider === "apikey") {
        await api.assistantStoreKey("gemini", choice.apiKey);
        toast("Gemini will use that API key until you quit. Send a message to start.");
        return;
      }
    } catch (e) {
      toast(String(e), "error");
      return;
    }
    if (choice.assistant === "claude" && choice.provider === "foundry" && choice.foundryApiKey) {
      toast("Microsoft Foundry is set for this session. Send a message to start.");
      return;
    }
    await this.openTerminal("login");
  }

  private async openTerminal(action: "login" | "resume" | "remote") {
    try {
      const wasRunning = this.running;
      const cmd = await api.claudeTerminal(
        action,
        this.sessionId ?? undefined,
        action === "login" ? undefined : this.chatId,
        action === "login" ? this.provider : undefined,
        action === "login" ? this.foundryResource || undefined : undefined,
        this.assistant,
      );
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
          this.statusLine.textContent = `${ev.model ?? assistantMeta(this.assistant).label}${paper ? ` · paper tools ${paper.status === "connected" ? "✓" : paper.status}` : ""}`;
          void this.loadSessions(this.sessionId ?? undefined);
        } else {
          const text = systemNote(ev);
          if (text) this.note(text.slice(0, 6000));
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
      case "control_request":
        this.onControl(ev);
        break;
      case "control_cancel_request":
        this.cancelPrompt(String(ev.request_id ?? ""));
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
      if (e.delta.text) this.streamedThisTurn = true;
      this.queueRender(this.currentMsg);
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onAssistant(message: any) {
    const id: string = message?.id ?? crypto.randomUUID();
    for (const block of message?.content ?? []) {
      if (block.type === "text") {
        const b = this.bubble(id);
        if (typeof block.text === "string" && block.text.trim()) this.streamedThisTurn = true;
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
  private onControl(ev: any) {
    const req = ev.request ?? {};
    if (req.subtype !== "can_use_tool") return;
    this.promptedThisTurn = true;
    if (req.tool_name === "AskUserQuestion") this.askQuestion(ev);
    else this.askPermission(ev);
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private askPermission(ev: any) {
    const req = ev.request ?? {};
    const requestId = String(ev.request_id ?? "");
    const tool = String(req.tool_name ?? "a tool");
    const detail = toolDetail(req.input);
    const reason = typeof req.decision_reason === "string" ? req.decision_reason : "";
    const status = h("span", { class: "perm-status muted" }, "");
    const actions = h("div", { class: "perm-actions" });
    const card = h(
      "div",
      { class: "perm", role: "group", "aria-label": `Allow ${tool}` },
      h("p", {}, h("strong", {}, `Allow ${tool}?`)),
      detail ? h("pre", { class: "perm-detail" }, detail) : null,
      reason ? h("p", { class: "muted small" }, reason) : null,
      actions,
      status,
    );
    const base = { request_id: requestId, tool_use_id: typeof req.tool_use_id === "string" ? req.tool_use_id : undefined };
    actions.append(
      h("button", { class: "primary small", onclick: () => void this.decide(card, { ...base, behavior: "allow", decision: "user_temporary" }, "Allowed") }, "Allow"),
    );
    if (!req.suppress_always_allow_rule && Array.isArray(req.permission_suggestions)) {
      for (const suggestion of req.permission_suggestions) {
        if (suggestion?.behavior !== "allow") continue;
        const label = suggestionLabel(suggestion);
        if (!label) continue;
        actions.append(
          h("button", {
            class: "small",
            onclick: () => void this.decide(card, {
              ...base,
              behavior: "allow",
              decision: "user_permanent",
              updated_permissions: [suggestion],
            }, "Allowed"),
          }, label),
        );
      }
    }
    actions.append(
      h("button", {
        class: "ghost small",
        onclick: () => void this.decide(card, { ...base, behavior: "deny", message: "The user denied this.", decision: "user_reject" }, "Denied"),
      }, "Deny"),
    );
    this.showPrompt(requestId, card);
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private askQuestion(ev: any) {
    const req = ev.request ?? {};
    const requestId = String(ev.request_id ?? "");
    const questions = Array.isArray(req.input?.questions) ? req.input.questions.filter((q: { question?: string }) => q?.question?.trim()) : [];
    const toolUseId = typeof req.tool_use_id === "string" ? req.tool_use_id : undefined;
    if (!requestId || !questions.length) {
      void this.decide(null, {
        request_id: requestId || "missing",
        behavior: "deny",
        message: "The question could not be shown.",
        tool_use_id: toolUseId,
        decision: "user_reject",
      }, "Dismissed");
      this.note("Claude asked a question the chat could not show, so it was dismissed.");
      return;
    }
    const picks = new Map<string, string[]>();
    const fields = questions.map((q: { question: string; header?: string; options?: { label?: string; description?: string }[]; multiSelect?: boolean }) => {
      const name = `q-${requestId}-${picks.size}`;
      picks.set(q.question, []);
      const options = Array.isArray(q.options) ? q.options.filter((o) => o?.label) : [];
      const choices = options.length
        ? h("div", { class: "perm-options" }, ...options.map((o) => {
            const input = h("input", { type: q.multiSelect ? "checkbox" : "radio", name, value: o.label }) as HTMLInputElement;
            input.addEventListener("change", () => {
              const chosen = [...document.getElementsByName(name)]
                .filter((el): el is HTMLInputElement => el instanceof HTMLInputElement && el.checked)
                .map((el) => el.value);
              picks.set(q.question, q.multiSelect ? chosen : chosen.slice(0, 1));
            });
            return h("label", { class: "check" }, input, o.label ?? "", o.description ? h("span", { class: "muted" }, ` ${o.description}`) : null);
          }))
        : (() => {
            const input = h("input", { type: "text" }) as HTMLInputElement;
            input.addEventListener("input", () => picks.set(q.question, input.value.trim() ? [input.value.trim()] : []));
            return input;
          })();
      return h("fieldset", {}, q.header ? h("legend", {}, q.header) : null, h("p", {}, q.question), choices);
    });
    const status = h("span", { class: "perm-status muted" }, "");
    const card = h(
      "div",
      { class: "perm", role: "group", "aria-label": "Claude asks" },
      h("p", {}, h("strong", {}, "Claude asks")),
      ...fields,
      h("div", { class: "perm-actions" },
        h("button", {
          class: "primary small",
          onclick: () => {
            const answers: Record<string, string | string[]> = {};
            for (const q of questions) {
              const chosen = picks.get(q.question) ?? [];
              if (!chosen.length) continue;
              answers[q.question] = q.multiSelect ? chosen : chosen[0];
            }
            if (!Object.keys(answers).length) {
              status.textContent = "Pick an answer first.";
              return;
            }
            void this.decide(card, {
              request_id: requestId,
              behavior: "allow",
              tool_use_id: toolUseId,
              decision: "user_temporary",
              updated_input: { questions: req.input.questions, answers },
            }, "Answered");
          },
        }, "Answer"),
        h("button", {
          class: "ghost small",
          onclick: () => void this.decide(card, {
            request_id: requestId,
            behavior: "deny",
            message: "The user dismissed the question.",
            tool_use_id: toolUseId,
            decision: "user_reject",
          }, "Dismissed"),
        }, "Dismiss"),
      ),
      status,
    );
    this.showPrompt(requestId, card);
  }

  private showPrompt(requestId: string, card: HTMLElement) {
    if (requestId) this.prompts.set(requestId, card);
    this.log.append(card);
    this.scroll();
  }

  private cancelPrompt(requestId: string) {
    const card = this.prompts.get(requestId);
    if (!card) return;
    card.querySelectorAll("button, input").forEach((el) => {
      (el as HTMLButtonElement).disabled = true;
    });
    const status = card.querySelector(".perm-status");
    if (status) status.textContent = "Cancelled";
    this.prompts.delete(requestId);
  }

  private async decide(card: HTMLElement | null, reply: ClaudePermissionReply, label: string) {
    const controls = card ? [...card.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button, input")] : [];
    controls.forEach((el) => (el.disabled = true));
    try {
      await api.claudePermission(this.chatId, reply);
      if (reply.request_id) this.prompts.delete(reply.request_id);
      const status = card?.querySelector(".perm-status");
      if (status) status.textContent = label;
    } catch (e) {
      controls.forEach((el) => (el.disabled = false));
      this.error(String(e));
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private onResult(ev: any) {
    const parts = [ev.is_error ? "Stopped with an error" : "Done"];
    if (typeof ev.total_cost_usd === "number") parts.push(`$${ev.total_cost_usd.toFixed(4)}`);
    if (ev.num_turns) parts.push(`${ev.num_turns} turns`);
    if (ev.duration_ms) parts.push(`${(ev.duration_ms / 1000).toFixed(1)}s`);
    this.note(parts.join(" · "));
    if (!ev.is_error && typeof ev.result === "string" && ev.result.trim() && !this.streamedThisTurn) {
      const id = `result-${crypto.randomUUID()}`;
      const bubble = this.bubble(id);
      bubble.text = ev.result;
      this.streamedThisTurn = true;
      this.queueRender(id);
    }
    if (ev.is_error && typeof ev.result === "string") {
      if (/log ?in|auth/i.test(ev.result)) this.loginNeeded(ev.result);
      else this.error(ev.result);
    }
    const denied = ev.permission_denials ?? [];
    if (denied.length && !this.promptedThisTurn) {
      const names = [...new Set(denied.map((d: { tool_name?: string }) => d.tool_name ?? "a tool"))].join(", ");
      this.note(`Claude was not allowed to use ${names}.`);
    }
    this.promptedThisTurn = false;
  }

  private loginNeeded(detail: string) {
    this.log.append(
      h("div", { class: "msg error" },
        h("p", {}, `${assistantMeta(this.assistant).product} needs you to sign in.`),
        h("pre", {}, detail.slice(-400)),
        h("button", { class: "primary small", onclick: () => void this.signIn() }, "Sign in…"),
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
