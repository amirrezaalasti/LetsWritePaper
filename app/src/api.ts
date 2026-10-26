// Typed wrappers around the Tauri commands in src-tauri/src/main.rs.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type AuthorKind = "human" | "agent" | "external" | "system";
export interface Author {
  id: string;
  kind: AuthorKind;
  name: string;
}

export interface ProjectInfo {
  root: string;
  title: string;
  format: "latex" | "typst";
  main: string;
  venue: string | null;
  documents: string[];
  owner: boolean;
  me: Author;
  severities: string[];
  categories: string[];
  personas: [string, string][];
}

export interface Section {
  level: number;
  title: string;
  label: string | null;
  line: number;
  end_line: number;
  start: number;
  end: number;
}

export interface LogEntry {
  first_id: number;
  last_id: number;
  doc: string;
  author: Author;
  session: string;
  messages: string[];
  added: number;
  removed: number;
  updates: number;
  started_at: number;
  ended_at: number;
}

export interface Change {
  id: number;
  doc: string;
  author: Author;
  summary: string;
  rationale: string | null;
  added: number;
  removed: number;
  created_at: number;
}

export interface Segment {
  kind: "equal" | "insert" | "delete";
  text: string;
}
export interface Hunk {
  old_line: number;
  new_line: number;
  segments: Segment[];
}
export interface DocDiff {
  path: string;
  status: "added" | "deleted" | "modified";
  words_added: number;
  words_removed: number;
  sections: { section: string | null; hunks: Hunk[] }[];
}

export interface BlameLine {
  line: number;
  text: string;
  author: string | null;
  authors: string[];
}

export interface Checkpoint {
  id: number;
  name: string;
  message: string;
  kind: string;
  update_id: number;
  author: Author;
  created_at: number;
  git_commit: string | null;
}

export interface Proposal {
  id: number;
  doc: string;
  old_text: string;
  new_text: string;
  rationale: string;
  author: Author;
  status: "pending" | "accepted" | "rejected" | "stale";
  created_at: number;
}

export interface Scores {
  soundness?: number;
  presentation?: number;
  contribution?: number;
  overall?: number;
  confidence?: number;
}

export interface ReviewRound {
  id: number;
  title: string;
  persona: string | null;
  venue: string | null;
  reviewer: Author;
  update_id: number;
  status: "open" | "finished";
  summary: string | null;
  scores: Scores | null;
  created_at: number;
}

export interface ReviewComment {
  id: number;
  round_id: number;
  doc: string;
  section: string | null;
  quote: string;
  offset_hint: number | null;
  severity: string;
  category: string;
  body: string;
  status: "open" | "addressed" | "disputed" | "wontfix";
  response: string | null;
  responder: Author | null;
  current_line: number | null;
}

export interface SkillInfo {
  name: string;
  description: string;
  kind: string;
  source: "project" | "user" | "claude" | "bundled";
  auto_sections: string[];
  global: boolean;
  bound_sections: string[];
  disabled: boolean;
}

export interface AppliedSkill {
  name: string;
  description: string;
  reason: string;
}

export interface Diagnostic {
  severity: "error" | "warning";
  file: string | null;
  line: number | null;
  message: string;
  hints?: string[];
}

export interface CompileResult {
  ok: boolean;
  engine: string;
  millis: number;
  diagnostics: Diagnostic[];
  page_count: number;
  svg_pages: string[];
  pdf: string | null;
  pdf_path: string | null;
}

export interface SyncStatus {
  link: string | null;
  connected: boolean;
  last_error: string | null;
  last_push_at: number | null;
  last_pull_at: number | null;
  pushed: number;
  pulled: number;
}

export interface ShareInfo {
  shared: boolean;
  link: string | null;
  hosted_here: boolean;
  status: SyncStatus | null;
}

export interface SetupStatus {
  name: string;
  setup_done: boolean;
  /** Typst preview is compiled into the desktop app. */
  typst: boolean;
  /** `tectonic`, `latexmk`, or absent. */
  latex: string | null;
  latex_path: string | null;
  claude_installed: boolean;
  claude_version: string | null;
  codex_installed: boolean;
  codex_version: string | null;
  gemini_installed: boolean;
  gemini_version: string | null;
  tectonic_command: string;
}

export interface ClaudeStatus {
  installed: boolean;
  path: string | null;
  version: string | null;
  install_command: string;
}

export interface ClaudeAuthInfo {
  logged_in: boolean;
  /** `claudeai`, `console`, `bedrock`, `vertex`, or `foundry`. */
  provider: string;
  detail: string;
}

export interface ClaudeSession {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  /** `claude`, `chatgpt`, or `gemini`. */
  assistant?: string;
}

export interface ClaudePermissionReply {
  request_id: string;
  behavior: "allow" | "deny";
  message?: string;
  tool_use_id?: string;
  updated_input?: unknown;
  updated_permissions?: unknown;
  decision?: "user_temporary" | "user_permanent" | "user_reject";
}

export interface EditorContext {
  path?: string;
  line?: number;
  section?: string;
  selection?: string;
}

export interface ChatAttachment {
  name: string;
  media_type: string;
  data: string;
}

export const api = {
  openProject: (path: string) => invoke<ProjectInfo>("open_project", { path }),
  initProject: (path: string, title: string, format: "latex" | "typst") =>
    invoke<ProjectInfo>("init_project", { path, title, format }),
  projectInfo: () => invoke<ProjectInfo>("project_info_cmd"),
  setupAgents: () => invoke<string[]>("setup_agents"),
  agentCommand: () => invoke<string>("agent_command"),

  docState: (path: string) => invoke<string>("doc_state", { path }),
  docText: (path: string) => invoke<string>("doc_text", { path }),
  pushUpdate: (path: string, update: string) => invoke<number>("push_update", { path, update }),
  createDocument: (path: string, content: string) => invoke<void>("create_document", { path, content }),
  deleteDocument: (path: string) => invoke<void>("delete_document", { path }),
  outline: (path: string) => invoke<Section[]>("outline", { path }),

  history: (path?: string, limit?: number) => invoke<LogEntry[]>("history", { path, limit }),
  changes: (path?: string, author?: string, limit?: number) => invoke<Change[]>("changes", { path, author, limit }),
  diff: (from: string, to: string, path?: string) => invoke<DocDiff[]>("diff", { from, to, path }),
  textAt: (path: string, version: string) => invoke<string | null>("text_at", { path, version }),
  blame: (path: string) =>
    invoke<{ lines: BlameLine[]; authors: Record<string, Author> }>("blame", { path }),
  checkpoints: () => invoke<Checkpoint[]>("checkpoints"),
  createCheckpoint: (name: string, message: string, kind?: string) =>
    invoke<Checkpoint>("create_checkpoint", { name, message, kind }),
  restore: (path: string, version: string) => invoke<number | null>("restore", { path, version }),

  proposals: (status?: string) => invoke<Proposal[]>("proposals", { status }),
  proposeEdit: (path: string, old_text: string, new_text: string, rationale: string) =>
    invoke<Proposal>("propose_edit", { path, old_text, new_text, rationale }),
  acceptProposal: (id: number) => invoke<{ line: number }>("accept_proposal", { id }),
  rejectProposal: (id: number) => invoke<void>("reject_proposal", { id }),

  reviewRounds: () => invoke<ReviewRound[]>("review_rounds"),
  reviewComments: (query?: { round_id?: number; status?: string; path?: string }) =>
    invoke<ReviewComment[]>("review_comments", { query }),
  startReview: (title: string, persona?: string, venue?: string) =>
    invoke<ReviewRound>("start_review", { title, persona, venue }),
  addReviewComment: (round_id: number, path: string, quote: string, severity: string, category: string, comment: string) =>
    invoke<ReviewComment>("add_review_comment", { round_id, path, quote, severity, category, comment }),
  finishReview: (round_id: number, summary: string, scores?: Scores) =>
    invoke<ReviewRound>("finish_review", { round_id, summary, scores }),
  respond: (id: number, response: string, status: string) =>
    invoke<ReviewComment>("respond_to_comment", { id, response, status }),
  responseLetter: () => invoke<string>("response_letter"),

  skills: () => invoke<SkillInfo[]>("skills"),
  skillBody: (name: string) => invoke<string>("skill_body", { name }),
  skillsForSection: (section: string) => invoke<AppliedSkill[]>("skills_for_section", { section }),
  bindSkill: (skill: string, section?: string) => invoke<void>("bind_skill", { skill, section }),
  unbindSkill: (skill: string, section?: string) => invoke<void>("unbind_skill", { skill, section }),
  installSkill: (path: string, user: boolean) => invoke<string>("install_skill", { path, user }),
  createSkill: (name: string, description: string, kind: string, sections: string[], body: string) =>
    invoke<string>("create_skill", { name, description, kind, sections, body }),

  shareInfo: () => invoke<ShareInfo>("share_info"),
  shareProject: (mode: "network" | "server", server?: string, secret?: string) =>
    invoke<ShareInfo>("share_project", { mode, server, secret }),
  stopSharing: () => invoke<void>("stop_sharing"),
  joinProject: (link: string, dir: string) => invoke<ProjectInfo>("join_project", { link, dir }),

  setupStatus: () => invoke<SetupStatus>("setup_status"),
  setupSave: (name: string, setup_done: boolean) => invoke<SetupStatus>("setup_save", { name, setup_done }),
  installTectonic: () => invoke<SetupStatus>("install_tectonic"),

  claudeStatus: (assistant?: string) => invoke<ClaudeStatus>("claude_status", { assistant }),
  claudeAuth: (assistant?: string) => invoke<ClaudeAuthInfo>("claude_auth", { assistant }),
  assistantStoreKey: (assistant: string, api_key: string) =>
    invoke<void>("assistant_store_key", { assistant, api_key }),
  claudeInstall: (assistant?: string) => invoke<ClaudeStatus>("claude_install", { assistant }),
  claudeSessions: () => invoke<ClaudeSession[]>("claude_sessions"),
  claudeStart: (
    chat_id: string,
    mode: "paper" | "edits",
    resume?: string,
    model?: string,
    provider?: string,
    foundry_resource?: string,
    foundry_api_key?: string,
    assistant?: string,
    api_key?: string,
  ) =>
    invoke<void>("claude_start", {
      chat_id,
      resume,
      mode,
      model,
      provider,
      foundry_resource,
      foundry_api_key,
      assistant,
      api_key,
    }),
  claudeSend: (chat_id: string, text: string, context?: EditorContext, attachments?: ChatAttachment[]) =>
    invoke<void>("claude_send", { chat_id, text, context, attachments }),
  claudePermission: (chat_id: string, reply: ClaudePermissionReply) =>
    invoke<void>("claude_permission", { chat_id, reply }),
  claudeStop: (chat_id: string) => invoke<string | null>("claude_stop", { chat_id }),
  claudeTerminal: (
    action: "login" | "resume" | "remote",
    session?: string,
    chat_id?: string,
    provider?: string,
    foundry_resource?: string,
    assistant?: string,
  ) => invoke<string>("claude_terminal", { action, session, chat_id, provider, foundry_resource, assistant }),

  compile: (main?: string) => invoke<CompileResult>("compile", { main }),
  exportPdf: () => invoke<string>("export_pdf"),
};

export const events = {
  onDocUpdate: (cb: (p: { path: string; update: string; author: Author }) => void): Promise<UnlistenFn> =>
    listen<{ path: string; update: string; author: Author }>("paper://doc-update", (e) => cb(e.payload)),
  onDocAdded: (cb: (path: string) => void) => listen<string>("paper://doc-added", (e) => cb(e.payload)),
  onDocRemoved: (cb: (path: string) => void) => listen<string>("paper://doc-removed", (e) => cb(e.payload)),
  onMetaChanged: (cb: () => void) => listen("paper://meta-changed", () => cb()),
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onClaude: (cb: (p: { chat_id: string; event: any }) => void) =>
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    listen<{ chat_id: string; event: any }>("claude://event", (e) => cb(e.payload)),
  onInstallLog: (cb: (line: string) => void) => listen<string>("claude://install-log", (e) => cb(e.payload)),
  onSetupLog: (cb: (line: string) => void) => listen<string>("setup://install-log", (e) => cb(e.payload)),
};

export function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function bytesToB64(bytes: Uint8Array): string {
  let bin = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(bin);
}
