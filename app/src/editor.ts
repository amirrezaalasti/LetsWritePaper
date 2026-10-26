// Editor tabs: one CodeMirror view per open document, each bound to its own
// Yjs replica that stays in sync with the Rust workspace.
import * as Y from "yjs";
import { Compartment, EditorState, StateEffect, StateField, RangeSetBuilder, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  GutterMarker,
  drawSelection,
  gutter,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { bracketMatching, HighlightStyle, StreamLanguage, syntaxHighlighting, type StreamParser } from "@codemirror/language";
import { stex } from "@codemirror/legacy-modes/mode/stex";
import { tags as t } from "@lezer/highlight";

import { api, b64ToBytes, bytesToB64, type Author, type BlameLine, type ReviewComment } from "./api";
import { REMOTE_ORIGIN, LOCAL_ORIGIN, UpdateBatcher, yTextBinding } from "./ybinding";
import { h, clear } from "./ui";

// ---- languages ---------------------------------------------------------------

const typstParser: StreamParser<{ math: boolean }> = {
  startState: () => ({ math: false }),
  token(stream, state) {
    if (state.math) {
      if (stream.skipTo("$")) {
        stream.next();
        state.math = false;
      } else stream.skipToEnd();
      return "string";
    }
    if (stream.sol() && stream.match(/^=+\s.*/)) return "heading";
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.match("$")) {
      state.math = true;
      return "string";
    }
    if (stream.match(/^#[A-Za-z_][\w-]*(\.[\w-]+)*/)) return "keyword";
    if (stream.match(/^<[\w:.-]+>/)) return "labelName";
    if (stream.match(/^@[\w:.-]+/)) return "link";
    if (stream.match(/^\*[^*\n]+\*/)) return "strong";
    if (stream.match(/^_[^_\n]+_/)) return "emphasis";
    if (stream.match(/^"(?:[^"\\]|\\.)*"/)) return "string";
    stream.next();
    return null;
  },
};

const bibParser: StreamParser<null> = {
  startState: () => null,
  token(stream) {
    if (stream.match(/^@\w+/)) return "keyword";
    if (stream.match(/^%.*/)) return "comment";
    if (stream.match(/^\w+(?=\s*=)/)) return "propertyName";
    if (stream.match(/^\{[\w:.-]+,/)) return "labelName";
    stream.next();
    return null;
  },
};

function languageFor(path: string): Extension {
  if (path.endsWith(".tex") || path.endsWith(".sty") || path.endsWith(".cls")) return StreamLanguage.define(stex);
  if (path.endsWith(".typ")) return StreamLanguage.define(typstParser);
  if (path.endsWith(".bib")) return StreamLanguage.define(bibParser);
  return [];
}

const paperHighlight = HighlightStyle.define([
  { tag: [t.heading, t.strong], fontWeight: "700", color: "var(--hl-heading)" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: [t.keyword, t.tagName], color: "var(--hl-keyword)" },
  { tag: [t.comment, t.lineComment], color: "var(--hl-comment)", fontStyle: "italic" },
  { tag: [t.string, t.special(t.string)], color: "var(--hl-math)" },
  { tag: [t.labelName, t.link, t.propertyName], color: "var(--hl-label)" },
  { tag: [t.bracket, t.atom], color: "var(--hl-bracket)" },
]);

// ---- review highlights -----------------------------------------------------------

const setReview = StateEffect.define<DecorationSet>();
const reviewField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const e of tr.effects) if (e.is(setReview)) deco = e.value;
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

function reviewDecorations(state: EditorState, comments: ReviewComment[]): DecorationSet {
  const text = state.doc.toString();
  const ranges: { from: number; to: number; c: ReviewComment }[] = [];
  for (const c of comments) {
    if (!c.quote) continue;
    const hint = c.current_line ? state.doc.line(Math.min(c.current_line, state.doc.lines)).from : 0;
    let best = -1;
    for (let i = text.indexOf(c.quote); i !== -1; i = text.indexOf(c.quote, i + 1)) {
      if (best === -1 || Math.abs(i - hint) < Math.abs(best - hint)) best = i;
    }
    if (best >= 0) ranges.push({ from: best, to: best + c.quote.length, c });
  }
  ranges.sort((a, b) => a.from - b.from || a.to - b.to);
  const b = new RangeSetBuilder<Decoration>();
  for (const r of ranges) {
    b.add(
      r.from,
      r.to,
      Decoration.mark({
        class: `cm-review cm-review-${r.c.severity}`,
        attributes: { title: `#${r.c.id} [${r.c.severity} · ${r.c.category}] ${r.c.body}` },
      }),
    );
  }
  return b.finish();
}

// ---- blame gutter ------------------------------------------------------------

class BlameMarker extends GutterMarker {
  constructor(
    private label: string,
    private kind: string,
    private title: string,
  ) {
    super();
  }
  toDOM() {
    return h("span", { class: `blame blame-${this.kind}`, title: this.title }, this.label);
  }
}

function blameGutter(lines: BlameLine[], authors: Record<string, Author>): Extension {
  const byLine = new Map(lines.map((l) => [l.line, l]));
  return gutter({
    class: "cm-blame-gutter",
    lineMarker(view, block) {
      const n = view.state.doc.lineAt(block.from).number;
      const l = byLine.get(n);
      if (!l?.author) return null;
      const a = authors[l.author];
      const name = a?.name ?? l.author;
      const others = l.authors.filter((x) => x !== l.author).map((x) => authors[x]?.name ?? x);
      return new BlameMarker(
        name.slice(0, 14),
        a?.kind ?? "external",
        `${name} (${a?.kind ?? "?"})${others.length ? ` · also ${others.join(", ")}` : ""}`,
      );
    },
  });
}

// ---- tabs --------------------------------------------------------------------

interface OpenDoc {
  path: string;
  ydoc: Y.Doc;
  view: EditorView;
  batcher: UpdateBatcher;
  blame: Compartment;
  host: HTMLElement;
}

export class Editors {
  private docs = new Map<string, OpenDoc>();
  active: string | null = null;
  private blameOn = false;
  onChange: (path: string) => void = () => {};
  onActivate: (path: string | null) => void = () => {};
  onCursor: (path: string, line: number) => void = () => {};

  constructor(
    private tabs: HTMLElement,
    private area: HTMLElement,
    private me: Author,
  ) {}

  async open(path: string, line?: number) {
    let d = this.docs.get(path);
    if (!d) {
      const ydoc = new Y.Doc();
      Y.applyUpdate(ydoc, b64ToBytes(await api.docState(path)), REMOTE_ORIGIN);
      const ytext = ydoc.getText("content");
      const batcher = new UpdateBatcher((u) => api.pushUpdate(path, bytesToB64(u)));
      ydoc.on("update", (update: Uint8Array, origin: unknown) => {
        if (origin === LOCAL_ORIGIN) batcher.push(update);
      });
      const blame = new Compartment();
      const host = h("div", { class: "editor-host" });
      const view = new EditorView({
        parent: host,
        state: EditorState.create({
          doc: ytext.toString(),
          extensions: [
            lineNumbers(),
            blame.of([]),
            highlightActiveLineGutter(),
            history(),
            drawSelection(),
            highlightActiveLine(),
            highlightSelectionMatches(),
            bracketMatching(),
            EditorView.lineWrapping,
            languageFor(path),
            syntaxHighlighting(paperHighlight),
            reviewField,
            keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
            yTextBinding(ytext, this.me.id),
            EditorView.updateListener.of((u) => {
              if (u.docChanged) this.onChange(path);
              if (u.selectionSet || u.docChanged) {
                this.onCursor(path, u.state.doc.lineAt(u.state.selection.main.head).number);
              }
            }),
          ],
        }),
      });
      d = { path, ydoc, view, batcher, blame, host };
      this.docs.set(path, d);
      this.area.append(host);
      if (this.blameOn) void this.refreshBlame(path);
    }
    this.activate(path);
    if (line) this.goto(line);
  }

  private activate(path: string) {
    this.active = path;
    for (const d of this.docs.values()) d.host.classList.toggle("hidden", d.path !== path);
    this.renderTabs();
    this.docs.get(path)?.view.focus();
    this.onActivate(path);
  }

  close(path: string) {
    const d = this.docs.get(path);
    if (!d) return;
    void d.batcher.flush();
    d.view.destroy();
    d.host.remove();
    d.ydoc.destroy();
    this.docs.delete(path);
    if (this.active === path) {
      const next = [...this.docs.keys()].pop() ?? null;
      if (next) this.activate(next);
      else {
        this.active = null;
        this.renderTabs();
        this.onActivate(null);
      }
    } else this.renderTabs();
  }

  closeAll() {
    for (const p of [...this.docs.keys()]) this.close(p);
  }

  private renderTabs() {
    clear(
      this.tabs,
      ...[...this.docs.keys()].map((p) =>
        h(
          "div",
          { class: `tab ${p === this.active ? "active" : ""}`, onclick: () => this.activate(p), title: p },
          h("span", {}, p.split("/").pop()!),
          h("button", {
            class: "tab-close",
            "aria-label": `Close ${p}`,
            onclick: (e: Event) => {
              e.stopPropagation();
              this.close(p);
            },
          }, "×"),
        ),
      ),
    );
  }

  /** Apply an update from the workspace (agent edit, another process, accepted proposal). */
  applyRemote(path: string, update: Uint8Array) {
    const d = this.docs.get(path);
    if (d) Y.applyUpdate(d.ydoc, update, REMOTE_ORIGIN);
  }

  isOpen(path: string) {
    return this.docs.has(path);
  }

  goto(line: number) {
    const d = this.active ? this.docs.get(this.active) : undefined;
    if (!d) return;
    const l = d.view.state.doc.line(Math.max(1, Math.min(line, d.view.state.doc.lines)));
    d.view.dispatch({ selection: { anchor: l.from }, effects: EditorView.scrollIntoView(l.from, { y: "center" }) });
    d.view.focus();
  }

  selectionText(): string {
    const d = this.active ? this.docs.get(this.active) : undefined;
    if (!d) return "";
    const { from, to } = d.view.state.selection.main;
    return d.view.state.sliceDoc(from, to);
  }

  text(path: string): string | null {
    return this.docs.get(path)?.view.state.doc.toString() ?? null;
  }

  async flushAll() {
    await Promise.all([...this.docs.values()].map((d) => d.batcher.flush()));
  }

  setReviewComments(comments: ReviewComment[]) {
    for (const d of this.docs.values()) {
      const mine = comments.filter((c) => c.doc === d.path && c.status === "open");
      d.view.dispatch({ effects: setReview.of(reviewDecorations(d.view.state, mine)) });
    }
  }

  get blameEnabled() {
    return this.blameOn;
  }

  async toggleBlame() {
    this.blameOn = !this.blameOn;
    for (const d of this.docs.values()) {
      if (this.blameOn) await this.refreshBlame(d.path);
      else d.view.dispatch({ effects: d.blame.reconfigure([]) });
    }
  }

  async refreshBlame(path: string) {
    const d = this.docs.get(path);
    if (!d || !this.blameOn) return;
    await d.batcher.flush();
    const { lines, authors } = await api.blame(path);
    d.view.dispatch({ effects: d.blame.reconfigure(blameGutter(lines, authors)) });
  }
}
