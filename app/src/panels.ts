// Sidebar panels: files, outline, history, review, skills, agents.
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { api, type Change, type DocDiff, type LogEntry, type ProjectInfo, type Proposal, type ReviewComment, type ReviewRound } from "./api";
import type { Editors } from "./editor";
import { authorBadge, clear, form, guard, h, showModal, timeAgo, toast } from "./ui";

export interface PanelContext {
  info: ProjectInfo;
  editors: Editors;
  /** Re-render the current panel. */
  refresh: () => void;
  openDoc: (path: string, line?: number) => Promise<void>;
}

export type PanelId = "files" | "outline" | "history" | "review" | "skills" | "agents" | "share";

export const PANELS: { id: PanelId; icon: string; label: string }[] = [
  { id: "files", icon: "▤", label: "Documents" },
  { id: "outline", icon: "☰", label: "Outline & section skills" },
  { id: "history", icon: "⟲", label: "Change log & checkpoints" },
  { id: "review", icon: "✎", label: "Peer review & suggestions" },
  { id: "skills", icon: "✦", label: "Skills" },
  { id: "agents", icon: "◆", label: "Agents" },
  { id: "share", icon: "⇄", label: "Share with other devices" },
];

export async function renderPanel(id: PanelId, el: HTMLElement, ctx: PanelContext) {
  switch (id) {
    case "files":
      return filesPanel(el, ctx);
    case "outline":
      return outlinePanel(el, ctx);
    case "history":
      return historyPanel(el, ctx);
    case "review":
      return reviewPanel(el, ctx);
    case "skills":
      return skillsPanel(el, ctx);
    case "agents":
      return agentsPanel(el, ctx);
    case "share":
      return sharePanel(el, ctx);
  }
}

function header(title: string, ...actions: HTMLElement[]) {
  return h("div", { class: "panel-header" }, h("h3", {}, title), h("span", { class: "spacer" }), ...actions);
}

function section(title: string, ...children: (Node | null)[]) {
  return h("section", { class: "panel-section" }, h("h4", {}, title), ...children.filter((c): c is Node => !!c));
}

// ---- files -------------------------------------------------------------------

async function filesPanel(el: HTMLElement, ctx: PanelContext) {
  const info = await api.projectInfo();
  const newFile = h("button", {
    class: "small",
    onclick: async () => {
      const ext = ctx.info.format === "typst" ? "typ" : "tex";
      const v = await form("New document", [{ name: "path", label: "Path", placeholder: `sections/intro.${ext}`, required: true }], "Create");
      if (!v) return;
      if (await guard(api.createDocument(v.path, ""))) {
        /* created; event refresh */
      }
      await ctx.openDoc(v.path.replace(/^\.\//, ""));
      ctx.refresh();
    },
  }, "+ New");
  clear(
    el,
    header(info.title, newFile),
    h("div", { class: "muted small-text" }, `${info.format === "typst" ? "Typst" : "LaTeX"} · main: ${info.main}`),
    h(
      "ul",
      { class: "list" },
      ...info.documents.map((d) =>
        h(
          "li",
          { class: `row ${ctx.editors.active === d ? "selected" : ""}`, onclick: () => ctx.openDoc(d) },
          h("span", { class: "file-icon" }, d.endsWith(".bib") ? "❝" : d === info.main ? "★" : "▫"),
          h("span", { class: "grow" }, d),
          h("button", {
            class: "icon hover-only",
            title: `Remove ${d} from the paper (kept in history)`,
            onclick: async (e: Event) => {
              e.stopPropagation();
              if (!confirm(`Remove ${d}? It stays in history and can be restored.`)) return;
              await guard(api.deleteDocument(d));
              ctx.editors.close(d);
              ctx.refresh();
            },
          }, "×"),
        ),
      ),
    ),
  );
}

// ---- outline + section skills -------------------------------------------------------

async function outlinePanel(el: HTMLElement, ctx: PanelContext) {
  const path = ctx.editors.active ?? ctx.info.main;
  const sections = (await guard(api.outline(path))) ?? [];
  const allSkills = (await guard(api.skills())) ?? [];
  const list = h("ul", { class: "list outline" });
  for (const s of sections) {
    const chips = h("div", { class: "chips" });
    const row = h(
      "li",
      { class: "outline-row", style: `padding-left:${(s.level - 1) * 12 + 6}px` },
      h(
        "div",
        { class: "row", onclick: () => ctx.openDoc(path, s.line) },
        h("span", { class: "grow" }, s.title),
        h("span", { class: "muted" }, `${s.line}`),
      ),
      chips,
    );
    list.append(row);
    void api.skillsForSection(s.title).then((applied) => {
      for (const a of applied) {
        const viaToml = a.reason.startsWith("paper.toml binds");
        chips.append(
          h(
            "span",
            { class: `chip ${a.reason.startsWith("global") ? "global" : ""}`, title: `${a.description}\n(${a.reason})` },
            a.name,
            !a.reason.startsWith("global")
              ? h("button", {
                  class: "chip-x",
                  "aria-label": `Remove ${a.name}`,
                  title: viaToml ? "Unbind from this section" : "Stop applying this skill automatically",
                  onclick: async () => {
                    const pattern = viaToml ? a.reason.match(/"(.+)"/)?.[1] : undefined;
                    await guard(api.unbindSkill(a.name, pattern ?? s.title.toLowerCase()));
                    ctx.refresh();
                  },
                }, "×")
              : null,
          ),
        );
      }
      chips.append(
        h("button", {
          class: "chip add",
          onclick: async () => {
            const v = await form(`Add a skill to “${s.title}”`, [
              { name: "skill", label: "Skill", type: "select", options: allSkills.map((k) => [k.name, `${k.name} — ${k.kind}`]) },
              { name: "pattern", label: "Apply to sections matching", value: s.title.toLowerCase() },
            ], "Bind");
            if (!v) return;
            await guard(api.bindSkill(v.skill, v.pattern));
            ctx.refresh();
          },
        }, "+ skill"),
      );
    });
  }
  clear(
    el,
    header("Outline"),
    h("div", { class: "muted small-text" }, `${path} — skills shown under each section are what agents follow there.`),
    sections.length ? list : h("p", { class: "muted" }, "No sections found."),
  );
}

// ---- history -------------------------------------------------------------------

export function renderDiffs(diffs: DocDiff[]): HTMLElement {
  if (!diffs.length) return h("p", { class: "muted" }, "No changes.");
  return h(
    "div",
    { class: "diff" },
    ...diffs.map((d) =>
      h(
        "div",
        { class: "diff-doc" },
        h("h4", {}, `${d.path} `, h("span", { class: "muted" }, `${d.status} · +${d.words_added} / −${d.words_removed} words`)),
        ...d.sections.map((s) =>
          h(
            "div",
            { class: "diff-section" },
            h("div", { class: "diff-section-title" }, `§ ${s.section ?? "(preamble)"}`),
            ...s.hunks.map((hk) =>
              h(
                "pre",
                { class: "hunk" },
                h("span", { class: "hunk-head" }, `line ${hk.new_line}\n`),
                ...hk.segments.map((seg) =>
                  seg.kind === "equal" ? document.createTextNode(seg.text) : h(seg.kind === "insert" ? "ins" : "del", {}, seg.text),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
}

async function showDiff(title: string, from: string, to: string, path?: string) {
  const diffs = await guard(api.diff(from, to, path));
  if (diffs) showModal(title, renderDiffs(diffs));
}

const MILESTONES: [string, string][] = [
  ["checkpoint", "Checkpoint"],
  ["submitted", "Milestone: submitted"],
  ["rebuttal", "Milestone: rebuttal"],
  ["camera-ready", "Milestone: camera-ready"],
  ["arxiv", "Milestone: arXiv"],
];

let historyFilter = { thisDoc: false, mine: false, sessions: false };

function dayLabel(ms: number): string {
  const d = new Date(ms);
  const start = (t: Date) => new Date(t.getFullYear(), t.getMonth(), t.getDate()).getTime();
  const diff = start(new Date()) - start(d);
  if (diff === 0) return "Today";
  if (diff === 86_400_000) return "Yesterday";
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  });
}

async function historyPanel(el: HTMLElement, ctx: PanelContext) {
  await ctx.editors.flushAll();
  const path = historyFilter.thisDoc && ctx.editors.active ? ctx.editors.active : undefined;
  const author = historyFilter.mine ? ctx.info.me.id : undefined;
  const [cps, changes, sessions] = await Promise.all([
    api.checkpoints(),
    historyFilter.sessions ? Promise.resolve([] as Change[]) : api.changes(path, author, 200),
    historyFilter.sessions ? api.history(path, 150) : Promise.resolve([] as LogEntry[]),
  ]);
  const sessionEntries = historyFilter.mine ? sessions.filter((e) => e.author.id === ctx.info.me.id) : sessions;
  const checkpointBtn = h("button", {
    class: "small primary",
    onclick: async () => {
      const v = await form("Save a version", [
        { name: "name", label: "Name", placeholder: "draft-2", required: true },
        { name: "message", label: "Description", type: "textarea", placeholder: "What state is the paper in?" },
        { name: "kind", label: "Type", type: "select", options: MILESTONES },
      ], "Save", "Checkpoints are named versions you can diff against and restore. In a git repository they are also commits that credit every human and agent contributor.");
      if (!v) return;
      await ctx.editors.flushAll();
      const cp = await guard(api.createCheckpoint(v.name, v.message, v.kind));
      if (cp) toast(`Saved “${cp.name}”${cp.git_commit ? ` (git ${cp.git_commit.slice(0, 8)})` : ""}`);
      ctx.refresh();
    },
  }, "Checkpoint");
  const compare = h("button", {
    class: "small",
    onclick: async () => {
      const options: [string, string][] = [["now", "Now"], ...cps.map((c): [string, string] => [c.name, `${c.name} (${c.kind})`]), ["start", "Empty project"]];
      const v = await form("Compare versions", [
        { name: "from", label: "From", type: "select", options: [...options.slice(1), options[0]] },
        { name: "to", label: "To", type: "select", options },
      ], "Compare");
      if (v) await showDiff(`${v.from} → ${v.to}`, v.from, v.to);
    },
  }, "Compare…");
  const blame = h("button", {
    class: `small ${ctx.editors.blameEnabled ? "on" : ""}`,
    title: "Show who wrote each line",
    onclick: async () => {
      await ctx.editors.toggleBlame();
      ctx.refresh();
    },
  }, "Blame");

  const toggle = (label: string, key: keyof typeof historyFilter) =>
    h("label", { class: "check" },
      (() => {
        const cb = h("input", { type: "checkbox" });
        cb.checked = historyFilter[key];
        cb.onchange = () => {
          historyFilter = { ...historyFilter, [key]: cb.checked };
          ctx.refresh();
        };
        return cb;
      })(),
      label,
    );

  const logBody = historyFilter.sessions ? sessionList(sessionEntries) : changeList(changes);

  clear(
    el,
    header("History", blame, compare, checkpointBtn),
    section(
      "Checkpoints & milestones",
      cps.length
        ? h(
            "ul",
            { class: "list" },
            ...cps.map((c) =>
              h(
                "li",
                { class: "row", title: c.message, onclick: () => showDiff(`${c.name} → now`, c.name, "now") },
                h("span", { class: `tag kind-${c.kind}` }, c.kind),
                h("span", { class: "grow" }, c.name),
                h("span", { class: "muted" }, timeAgo(c.created_at)),
                ctx.editors.active
                  ? h("button", {
                      class: "icon hover-only",
                      title: `Restore ${ctx.editors.active} to ${c.name}`,
                      onclick: async (e: Event) => {
                        e.stopPropagation();
                        const p = ctx.editors.active!;
                        if (!confirm(`Restore ${p} to “${c.name}”? This is recorded as a new edit; nothing is lost.`)) return;
                        await guard(api.restore(p, c.name));
                      },
                    }, "↺")
                  : null,
              ),
            ),
          )
        : h("p", { class: "muted" }, "No checkpoints yet."),
    ),
    section(
      "Change log",
      h("p", { class: "muted small-text" }, "Every edit is saved on its own. Click an entry to see the exact change."),
      h("div", { class: "filters" }, toggle("This document", "thisDoc"), toggle("My edits", "mine"), toggle("Group into sessions", "sessions")),
      logBody,
    ),
  );
}

function changeList(changes: Change[]): HTMLElement {
  if (!changes.length) return h("p", { class: "muted" }, "No changes yet.");
  const groups: { label: string; items: Change[] }[] = [];
  for (const c of changes) {
    const label = dayLabel(c.created_at);
    const last = groups[groups.length - 1];
    if (last?.label === label) last.items.push(c);
    else groups.push({ label, items: [c] });
  }
  return h(
    "div",
    {},
    ...groups.map((g) =>
      h(
        "div",
        {},
        h("div", { class: "day-label" }, g.label),
        h("ul", { class: "list change-list" }, ...g.items.map((c) => changeRow(c))),
      ),
    ),
  );
}

function changeRow(c: Change): HTMLElement {
  return h(
    "li",
    {
      class: "activity-row",
      title: "Show this change",
      onclick: () => showDiff(`${c.author.name} · ${c.doc}`, `#${Math.max(c.id - 1, 0)}`, `#${c.id}`, c.doc),
    },
    h("div", { class: "row-top" }, authorBadge(c.author), h("span", { class: "spacer" }), h("span", { class: "muted" }, timeAgo(c.created_at))),
    h("div", { class: "row-mid" }, h("span", { class: "grow" }, c.doc), h("span", { class: "plus" }, `+${c.added}`), h("span", { class: "minus" }, `−${c.removed}`)),
    h("div", { class: "row-msg" }, ...summaryParts(c.summary)),
    c.rationale ? h("div", { class: "row-why" }, c.rationale) : null,
  );
}

function summaryParts(summary: string): (Node | string)[] {
  const arrow = summary.indexOf(" → ");
  if (arrow !== -1) {
    return [h("span", { class: "chg-del" }, summary.slice(0, arrow)), " → ", h("span", { class: "chg-add" }, summary.slice(arrow + 3))];
  }
  const added = summary.match(/^(.*?\badded )([\s\S]*)$/);
  if (added) return [added[1], h("span", { class: "chg-add" }, added[2])];
  const removed = summary.match(/^(.*?\bremoved )([\s\S]*)$/);
  if (removed) return [removed[1], h("span", { class: "chg-del" }, removed[2])];
  return [summary];
}

function sessionList(entries: LogEntry[]): HTMLElement {
  if (!entries.length) return h("p", { class: "muted" }, "No sessions yet.");
  return h("ul", { class: "list change-list" }, ...entries.map((e) => activityRow(e)));
}

function activityRow(e: LogEntry): HTMLElement {
  return h(
    "li",
    {
      class: "activity-row",
      onclick: () => showDiff(`${e.author.name} · ${e.doc}`, `#${e.first_id - 1}`, `#${e.last_id}`, e.doc),
    },
    h("div", { class: "row-top" }, authorBadge(e.author), h("span", { class: "spacer" }), h("span", { class: "muted" }, timeAgo(e.ended_at))),
    h("div", { class: "row-mid" }, h("span", {}, e.doc), " ", h("span", { class: "plus" }, `+${e.added}`), " ", h("span", { class: "minus" }, `−${e.removed}`)),
    e.messages.length ? h("div", { class: "row-msg" }, e.messages.join(" · ")) : null,
  );
}

// ---- review & proposals ------------------------------------------------------------

let reviewShowAll = false;

async function reviewPanel(el: HTMLElement, ctx: PanelContext) {
  const [rounds, comments, proposals] = await Promise.all([api.reviewRounds(), api.reviewComments(), api.proposals()]);
  const me = ctx.info.me;
  const myOpenRounds = rounds.filter((r) => r.status === "open" && r.reviewer.id === me.id);

  const start = h("button", {
    class: "small",
    onclick: async () => {
      const v = await form("Start a review round", [
        { name: "title", label: "Title", value: "Internal review", required: true },
        { name: "persona", label: "Persona (optional)", type: "select", options: [["", "—"], ...ctx.info.personas.map(([p, d]): [string, string] => [p, `${p} — ${d}`])] },
        { name: "venue", label: "Venue", value: ctx.info.venue ?? "" },
      ], "Start", "You review the current version; select text in the editor and use “Comment on selection”.");
      if (!v) return;
      await guard(api.startReview(v.title, v.persona || undefined, v.venue || undefined));
      ctx.refresh();
    },
  }, "Start review");

  const commentBtn = h("button", {
    class: "small",
    disabled: !myOpenRounds.length,
    title: myOpenRounds.length ? "Anchor a comment to the selected text" : "Start a review round first",
    onclick: async () => {
      const quote = ctx.editors.selectionText().trim();
      const path = ctx.editors.active;
      if (!quote || !path) return toast("Select the text you want to comment on.", "error");
      const v = await form("Review comment", [
        { name: "round", label: "Round", type: "select", options: myOpenRounds.map((r): [string, string] => [String(r.id), r.title]) },
        { name: "severity", label: "Severity", type: "select", options: ctx.info.severities.map((s): [string, string] => [s, s]) },
        { name: "category", label: "Category", type: "select", options: ctx.info.categories.map((s): [string, string] => [s, s]) },
        { name: "comment", label: "Comment", type: "textarea", required: true },
      ], "Add", `On: “${quote.slice(0, 140)}${quote.length > 140 ? "…" : ""}”`);
      if (!v) return;
      await ctx.editors.flushAll();
      await guard(api.addReviewComment(Number(v.round), path, quote, v.severity, v.category, v.comment));
      ctx.refresh();
    },
  }, "Comment on selection");

  const letter = h("button", {
    class: "small",
    onclick: async () => {
      const md = await guard(api.responseLetter());
      if (md === undefined) return;
      showModal("Response to reviewers", h("pre", { class: "letter" }, md), [["Copy", () => void navigator.clipboard.writeText(md)]]);
    },
  }, "Response letter");

  const pending = proposals.filter((p) => p.status === "pending" || p.status === "stale");
  const visibleComments = reviewShowAll ? comments : comments.filter((c) => c.status === "open");

  clear(
    el,
    header("Review", start, commentBtn, letter),
    section(
      `Suggested edits (${pending.length})`,
      pending.length ? h("div", {}, ...pending.map((p) => proposalCard(p, ctx))) : h("p", { class: "muted" }, "No pending suggestions. Agents can propose edits with propose_edit."),
    ),
    section(
      `Review comments (${comments.filter((c) => c.status === "open").length} open)`,
      h("label", { class: "check" },
        (() => {
          const cb = h("input", { type: "checkbox" });
          cb.checked = reviewShowAll;
          cb.onchange = () => {
            reviewShowAll = cb.checked;
            ctx.refresh();
          };
          return cb;
        })(),
        "Show resolved",
      ),
      ...rounds.map((r) => roundCard(r, visibleComments.filter((c) => c.round_id === r.id), ctx)),
      rounds.length ? null : h("p", { class: "muted" }, "No reviews yet. Start one yourself, or ask an agent (see Agents)."),
    ),
  );
  ctx.editors.setReviewComments(comments);
}

function proposalCard(p: Proposal, ctx: PanelContext): HTMLElement {
  return h(
    "div",
    { class: `card ${p.status}` },
    h("div", { class: "row-top" }, authorBadge(p.author), h("span", { class: "spacer" }), h("span", { class: "muted" }, `#${p.id} · ${p.doc}`)),
    h("div", { class: "row-msg" }, p.rationale),
    h("pre", { class: "hunk" }, h("del", {}, p.old_text), "\n", h("ins", {}, p.new_text)),
    p.status === "stale"
      ? h("div", { class: "muted" }, "The original text has changed since this was proposed.", h("button", { class: "small", onclick: async () => (await guard(api.rejectProposal(p.id)), ctx.refresh()) }, "Dismiss"))
      : h(
          "div",
          { class: "actions" },
          h("button", {
            class: "small primary",
            onclick: async () => {
              await ctx.editors.flushAll();
              const out = await guard(api.acceptProposal(p.id));
              if (out) await ctx.openDoc(p.doc, out.line);
              ctx.refresh();
            },
          }, "Accept"),
          h("button", { class: "small", onclick: async () => (await guard(api.rejectProposal(p.id)), ctx.refresh()) }, "Reject"),
          h("button", { class: "small ghost", onclick: () => ctx.openDoc(p.doc) }, "Open"),
        ),
  );
}

function scoreLine(r: ReviewRound): string {
  const s = r.scores;
  if (!s) return "";
  const parts = Object.entries(s).filter(([, v]) => v !== undefined && v !== null).map(([k, v]) => `${k} ${v}`);
  return parts.join(" · ");
}

function roundCard(r: ReviewRound, comments: ReviewComment[], ctx: PanelContext): HTMLElement {
  const mine = r.reviewer.id === ctx.info.me.id;
  return h(
    "details",
    { class: "card round", open: r.status === "open" || comments.length > 0 },
    h(
      "summary",
      {},
      h("strong", {}, r.title),
      " ",
      authorBadge(r.reviewer),
      r.persona ? h("span", { class: "tag" }, r.persona) : null,
      h("span", { class: `tag status-${r.status}` }, r.status),
    ),
    r.summary ? h("p", { class: "summary" }, r.summary) : null,
    r.scores ? h("div", { class: "muted" }, scoreLine(r)) : null,
    ...comments.map((c) => commentCard(c, ctx)),
    r.status === "open" && mine
      ? h("button", {
          class: "small",
          onclick: async () => {
            const v = await form("Finish review", [
              { name: "summary", label: "Summary", type: "textarea", required: true },
              { name: "soundness", label: "Soundness (1-4)", type: "number" },
              { name: "presentation", label: "Presentation (1-4)", type: "number" },
              { name: "contribution", label: "Contribution (1-4)", type: "number" },
              { name: "overall", label: "Overall (1-10)", type: "number" },
              { name: "confidence", label: "Confidence (1-5)", type: "number" },
            ], "Finish");
            if (!v) return;
            const num = (k: string) => (v[k] ? Number(v[k]) : undefined);
            await guard(api.finishReview(r.id, v.summary, {
              soundness: num("soundness"), presentation: num("presentation"), contribution: num("contribution"), overall: num("overall"), confidence: num("confidence"),
            }));
            ctx.refresh();
          },
        }, "Finish round")
      : null,
  );
}

function commentCard(c: ReviewComment, ctx: PanelContext): HTMLElement {
  return h(
    "div",
    { class: `comment sev-${c.severity} status-${c.status}` },
    h(
      "div",
      { class: "row-top" },
      h("span", { class: `tag sev ${c.severity}` }, c.severity),
      h("span", { class: "tag" }, c.category),
      h("span", { class: "spacer" }),
      h("span", { class: "muted" }, `#${c.id} · ${c.section ?? c.doc}`),
    ),
    h(
      "blockquote",
      {
        class: c.current_line ? "jump" : "stale",
        title: c.current_line ? `Go to ${c.doc}:${c.current_line}` : "The quoted text has since been rewritten",
        onclick: () => c.current_line && ctx.openDoc(c.doc, c.current_line),
      },
      c.quote,
    ),
    h("p", {}, c.body),
    c.response ? h("p", { class: "response" }, "↳ ", c.response, c.responder ? h("span", { class: "muted" }, ` — ${c.responder.name}`) : null) : null,
    h(
      "div",
      { class: "actions" },
      h("button", {
        class: "small",
        onclick: async () => {
          const v = await form(`Respond to #${c.id}`, [
            { name: "response", label: "Response", type: "textarea", value: c.response ?? "", required: true },
            { name: "status", label: "Status", type: "select", options: [["addressed", "Addressed"], ["disputed", "Disputed"], ["wontfix", "Won't fix"], ["open", "Open"]] },
          ], "Save", c.body);
          if (!v) return;
          await guard(api.respond(c.id, v.response, v.status));
          ctx.refresh();
        },
      }, c.response ? "Edit response" : "Respond"),
    ),
  );
}

// ---- skills --------------------------------------------------------------------

async function skillsPanel(el: HTMLElement, ctx: PanelContext) {
  const skills = await api.skills();
  const create = h("button", {
    class: "small primary",
    onclick: async () => {
      const v = await form("New skill", [
        { name: "name", label: "Name", placeholder: "venue-style", required: true },
        { name: "description", label: "When should agents use it?", required: true },
        { name: "kind", label: "Kind", type: "select", options: [["writing", "Writing"], ["style", "Style"], ["review", "Review"]] },
        { name: "sections", label: "Sections (comma-separated globs, optional)", placeholder: "intro*, conclusion" },
        { name: "body", label: "Instructions (Markdown)", type: "textarea", required: true },
      ], "Create", "Skills are SKILL.md files saved in .paper/skills, versioned with the paper and compatible with Claude Code skills.");
      if (!v) return;
      const sections = v.sections.split(",").map((s) => s.trim()).filter(Boolean);
      const p = await guard(api.createSkill(v.name, v.description, v.kind, sections, v.body));
      if (p) toast(`Created ${p}`);
      ctx.refresh();
    },
  }, "+ New");
  const install = h("button", {
    class: "small",
    onclick: async () => {
      const dir = await openDialog({ directory: true, title: "Choose a skill folder (containing SKILL.md)" });
      if (typeof dir !== "string") return;
      const user = confirm("Install for all your papers? (Cancel = this paper only)");
      const name = await guard(api.installSkill(dir, user));
      if (name) toast(`Installed ${name}`);
      ctx.refresh();
    },
  }, "Install…");

  const groups: Record<string, typeof skills> = {};
  for (const s of skills) (groups[s.kind] ??= []).push(s);
  clear(
    el,
    header("Skills", install, create),
    h("p", { class: "muted small-text" }, "Skills guide agents section by section. Sources: bundled, your Claude Code skills (~/.claude/skills), your user skills, and this paper's .paper/skills."),
    ...Object.entries(groups).map(([kind, list]) =>
      section(
        kind[0].toUpperCase() + kind.slice(1),
        ...list.map((s) =>
          h(
            "div",
            { class: `card skill ${s.disabled ? "disabled" : ""}` },
            h("div", { class: "row-top" }, h("strong", {}, s.name), h("span", { class: `tag src-${s.source}` }, s.source), h("span", { class: "spacer" }),
              h("label", { class: "check", title: "Apply to every section" },
                (() => {
                  const cb = h("input", { type: "checkbox" });
                  cb.checked = s.global;
                  cb.onchange = async () => {
                    await guard(cb.checked ? api.bindSkill(s.name) : api.unbindSkill(s.name));
                    ctx.refresh();
                  };
                  return cb;
                })(),
                "everywhere",
              ),
            ),
            h("div", { class: "muted" }, s.description),
            h(
              "div",
              { class: "chips" },
              ...s.bound_sections.map((p) => h("span", { class: "chip" }, `§ ${p}`)),
              ...(s.disabled ? [] : s.auto_sections.map((p) => h("span", { class: "chip auto", title: "Matched automatically" }, `auto: ${p}`))),
            ),
            h(
              "div",
              { class: "actions" },
              h("button", {
                class: "small ghost",
                onclick: async () => {
                  const body = await guard(api.skillBody(s.name));
                  if (body !== undefined) showModal(s.name, h("pre", { class: "skill-body" }, body));
                },
              }, "View"),
              h("button", {
                class: "small ghost",
                onclick: async () => {
                  const v = await form(`Bind ${s.name}`, [{ name: "pattern", label: "Section title pattern", placeholder: "intro*", required: true }], "Bind");
                  if (!v) return;
                  await guard(api.bindSkill(s.name, v.pattern));
                  ctx.refresh();
                },
              }, "Bind to section…"),
            ),
          ),
        ),
      ),
    ),
  );
}

// ---- agents --------------------------------------------------------------------

async function agentsPanel(el: HTMLElement, _ctx: PanelContext) {
  const [cmd, log] = await Promise.all([api.agentCommand(), api.history(undefined, 200)]);
  const agentLog = log.filter((e) => e.author.kind === "agent").slice(0, 25);
  const copy = (text: string) => h("button", { class: "small", onclick: () => (navigator.clipboard.writeText(text), toast("Copied")) }, "Copy");
  const prompts: [string, string][] = [
    ["/mcp__paper__write-section", "Draft or revise a section using its skills"],
    ["/mcp__paper__review-paper", "Mock peer review with a reviewer persona"],
    ["/mcp__paper__address-reviews", "Work through open review comments"],
  ];
  clear(
    el,
    header("Agents"),
    section(
      "Connect Claude Code / Claude in VS Code",
      h("p", { class: "muted" }, "Agents use the paper's MCP server. Their edits appear live here, credited to them in history and blame."),
      h("button", {
        class: "primary",
        onclick: async () => {
          const files = await guard(api.setupAgents());
          if (files) toast(`Wrote ${files.join(", ")}`);
        },
      }, "Set up this project for agents"),
      h("p", { class: "muted small-text" }, "Writes .mcp.json, a Claude Code hook that credits direct file edits to the agent, and AGENTS.md."),
      h("div", { class: "code-row" }, h("code", {}, cmd), copy(cmd)),
      h("p", { class: "muted small-text" }, "Or register the server globally with the command above. Any MCP client works (Cursor, Claude Desktop, …): command = this app, args = [\"mcp\"], run in the project folder."),
    ),
    section(
      "Prompts (slash commands in Claude Code)",
      h("ul", { class: "list" }, ...prompts.map(([c, d]) => h("li", { class: "row" }, h("code", { class: "grow" }, c), h("span", { class: "muted" }, d), copy(c)))),
    ),
    section("Recent agent activity", agentLog.length ? h("ul", { class: "list change-list" }, ...agentLog.map(activityRow)) : h("p", { class: "muted" }, "No agent edits yet.")),
  );
}

// ---- share across devices -----------------------------------------------------------

async function sharePanel(el: HTMLElement, ctx: PanelContext) {
  const info = await api.shareInfo();
  const copy = (text: string) => h("button", { class: "small", onclick: () => (navigator.clipboard.writeText(text), toast("Link copied")) }, "Copy");
  if (!info.shared) {
    const network = h("button", {
      class: "primary",
      onclick: async () => {
        network.disabled = true;
        network.textContent = "Sharing…";
        await guard(api.shareProject("network"));
        ctx.refresh();
      },
    }, "Share on this network");
    const viaServer = h("button", {
      onclick: async () => {
        const v = await form("Share through a relay server", [
          { name: "server", label: "Relay URL", placeholder: "https://relay.example.org", required: true },
          { name: "secret", label: "Relay secret (if the relay requires one)" },
        ], "Share", "Run a relay anywhere with `paper serve --secret …` (see README), then every device can reach it from anywhere.");
        if (!v) return;
        await guard(api.shareProject("server", v.server, v.secret));
        ctx.refresh();
      },
    }, "Share via relay server…");
    clear(
      el,
      header("Share"),
      section(
        "Work on this paper from another device",
        h("p", { class: "muted" }, "Sharing keeps a full copy on each device: edits, history, reviews, skills and checkpoints sync live and merge automatically, even after working offline."),
        h("div", { class: "actions" }, network, viaServer),
        h("p", { class: "muted small-text" }, "“This network” hosts the relay inside this app: devices on the same Wi-Fi/LAN can join while it runs. For anywhere-access, use a relay server."),
      ),
      section(
        "Continue a Claude chat elsewhere",
        h("p", { class: "muted" }, "Open the Claude panel and use 📱 to hand the conversation to your phone or another computer through Claude Code's Remote Control."),
      ),
    );
    return;
  }
  const s = info.status;
  const state = !s ? "starting…" : s.last_error ? `offline — ${s.last_error}` : s.connected ? "live" : "connecting…";
  clear(
    el,
    header("Share"),
    section(
      "Shared",
      h("p", {}, h("span", { class: `dot ${s?.connected ? "ok" : "bad"}` }), ` Sync ${state}`),
      s ? h("p", { class: "muted small-text" }, `Sent ${s.pushed} · received ${s.pulled} changes${s.last_pull_at ? ` · last update ${timeAgo(s.last_pull_at)}` : ""}`) : null,
      info.link ? h("div", { class: "code-row" }, h("code", {}, info.link), copy(info.link)) : null,
      h("p", { class: "muted small-text" }, "On the other device: open LetsWritePaper → “Join a shared paper…” and paste the link, or run `paper join '<link>'`. Anyone with the link can edit, so share it like a password."),
      info.hosted_here ? h("p", { class: "muted small-text" }, "This computer hosts the relay: keep the app open while others work.") : null,
      h("button", {
        class: "small",
        onclick: async () => {
          if (!confirm("Stop syncing this copy? Nothing is deleted; you can share again later.")) return;
          await guard(api.stopSharing());
          ctx.refresh();
        },
      }, "Stop syncing on this device"),
    ),
  );
}
