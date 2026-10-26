// Two-way binding between a CodeMirror view and a Y.Text.
//
// Local edits are written into the Y.Text with an `a` (author) attribute so
// the Rust side can compute blame; remote updates (agents, other processes)
// arrive as Yjs updates and are replayed into the editor without echoing.
import * as Y from "yjs";
import { Annotation, type ChangeSpec, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";

export const LOCAL_ORIGIN = "local";
export const REMOTE_ORIGIN = "remote";
const fromRemote = Annotation.define<boolean>();

export function yTextBinding(ytext: Y.Text, authorId: string): Extension {
  return ViewPlugin.fromClass(
    class {
      private observer: (event: Y.YTextEvent, tr: Y.Transaction) => void;

      constructor(private view: EditorView) {
        this.observer = (event, tr) => {
          if (tr.origin === LOCAL_ORIGIN) return;
          // Delta positions are sequential; CodeMirror wants original-document positions.
          const changes: ChangeSpec[] = [];
          let pos = 0;
          for (const op of event.delta) {
            if (op.retain != null) {
              pos += op.retain;
            } else if (op.delete != null) {
              changes.push({ from: pos, to: pos + op.delete });
              pos += op.delete;
            } else if (typeof op.insert === "string") {
              changes.push({ from: pos, insert: op.insert });
            }
          }
          if (changes.length) {
            this.view.dispatch({ changes, annotations: [fromRemote.of(true)] });
          }
        };
        ytext.observe(this.observer);
      }

      update(update: ViewUpdate) {
        if (!update.docChanged) return;
        if (update.transactions.every((tr) => tr.annotation(fromRemote))) return;
        const attrs = { a: authorId };
        ytext.doc!.transact(() => {
          update.changes.iterChanges((fromA, toA, fromB, _toB, inserted) => {
            if (toA > fromA) ytext.delete(fromB, toA - fromA);
            if (inserted.length) ytext.insert(fromB, inserted.toString(), attrs);
          });
        }, LOCAL_ORIGIN);
      }

      destroy() {
        ytext.unobserve(this.observer);
      }
    },
  );
}

/**
 * Collects local Yjs updates and ships them in small batches, so typing a
 * sentence becomes one history entry rather than dozens.
 */
export class UpdateBatcher {
  private pending: Uint8Array[] = [];
  private timer: number | undefined;

  constructor(
    private send: (merged: Uint8Array) => Promise<unknown>,
    private delayMs = 400,
  ) {}

  push(update: Uint8Array) {
    this.pending.push(update);
    if (this.timer === undefined) {
      this.timer = window.setTimeout(() => this.flush(), this.delayMs);
    }
  }

  async flush() {
    window.clearTimeout(this.timer);
    this.timer = undefined;
    if (!this.pending.length) return;
    const merged = Y.mergeUpdates(this.pending);
    this.pending = [];
    try {
      await this.send(merged);
    } catch (e) {
      console.error("failed to sync update", e);
    }
  }
}
