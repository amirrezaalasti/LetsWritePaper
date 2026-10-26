// Live preview: Typst renders to SVG pages in-process (instant); LaTeX renders
// the PDF produced by tectonic/latexmk with pdf.js.
import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

import { api, b64ToBytes, type CompileResult, type Diagnostic } from "./api";
import { h, clear } from "./ui";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export class Preview {
  private pages: HTMLElement;
  private diags: HTMLElement;
  private status: HTMLElement;
  private running = false;
  private queued = false;
  private timer: number | undefined;
  private zoom = 1;
  onDiagnosticClick: (d: Diagnostic) => void = () => {};
  onStatus: (text: string, ok: boolean | null) => void = () => {};
  /** Runs before each compile, e.g. to flush unsent keystrokes to the workspace. */
  beforeCompile: () => Promise<unknown> = async () => {};

  constructor(private root: HTMLElement) {
    this.status = h("span", { class: "muted" }, "Not compiled");
    this.pages = h("div", { class: "preview-pages" });
    this.diags = h("div", { class: "diagnostics" });
    clear(
      root,
      h(
        "div",
        { class: "pane-header" },
        h("strong", {}, "Preview"),
        this.status,
        h("span", { class: "spacer" }),
        h("button", { class: "icon", title: "Zoom out", onclick: () => this.setZoom(this.zoom - 0.1) }, "−"),
        h("button", { class: "icon", title: "Zoom in", onclick: () => this.setZoom(this.zoom + 0.1) }, "+"),
        h("button", { title: "Compile (⌘/Ctrl+S)", onclick: () => void this.compile() }, "Compile"),
      ),
      this.diags,
      this.pages,
    );
  }

  private setZoom(z: number) {
    this.zoom = Math.min(2.5, Math.max(0.4, z));
    this.pages.style.setProperty("--zoom", String(this.zoom));
  }

  /** Debounced compile, used while typing (Typst only: LaTeX is too slow). */
  schedule(delay = 500) {
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.compile(), delay);
  }

  async compile() {
    if (this.running) {
      this.queued = true;
      return;
    }
    this.running = true;
    this.status.textContent = "Compiling…";
    this.onStatus("Compiling…", null);
    try {
      await this.beforeCompile();
      const r = await api.compile();
      await this.render(r);
    } catch (e) {
      this.status.textContent = "Failed";
      clear(this.diags, h("div", { class: "diag error" }, String(e)));
    } finally {
      this.running = false;
      if (this.queued) {
        this.queued = false;
        void this.compile();
      }
    }
  }

  private async render(r: CompileResult) {
    const errors = r.diagnostics.filter((d) => d.severity === "error").length;
    const warnings = r.diagnostics.length - errors;
    const summary = r.ok
      ? `${r.engine} · ${r.millis} ms${r.page_count ? ` · ${r.page_count} pages` : ""}${warnings ? ` · ${warnings} warnings` : ""}`
      : `${errors} error${errors === 1 ? "" : "s"}`;
    this.status.textContent = summary;
    this.onStatus(summary, r.ok);
    clear(
      this.diags,
      ...r.diagnostics.slice(0, 50).map((d) =>
        h(
          "button",
          { class: `diag ${d.severity}`, onclick: () => this.onDiagnosticClick(d) },
          h("span", { class: "diag-loc" }, `${d.file ?? "?"}:${d.line ?? "?"}`),
          ` ${d.message}`,
        ),
      ),
    );
    // Keep the last good render visible while the document has errors.
    if (r.svg_pages.length) {
      const scroll = this.pages.scrollTop;
      clear(this.pages, ...r.svg_pages.map((svg) => {
        const page = h("div", { class: "page" });
        page.innerHTML = svg;
        return page;
      }));
      this.pages.scrollTop = scroll;
    } else if (r.pdf) {
      await this.renderPdf(b64ToBytes(r.pdf));
    }
  }

  private async renderPdf(bytes: Uint8Array) {
    const scroll = this.pages.scrollTop;
    const doc = await pdfjs.getDocument({ data: bytes }).promise;
    const canvases: HTMLElement[] = [];
    const ratio = window.devicePixelRatio || 1;
    for (let i = 1; i <= doc.numPages; i++) {
      const page = await doc.getPage(i);
      const viewport = page.getViewport({ scale: 1.5 * ratio });
      const canvas = h("canvas", { width: viewport.width, height: viewport.height });
      await page.render({ canvas, viewport }).promise;
      canvases.push(h("div", { class: "page" }, canvas));
    }
    clear(this.pages, ...canvases);
    this.pages.scrollTop = scroll;
  }

  reset() {
    clear(this.pages);
    clear(this.diags);
    this.status.textContent = "Not compiled";
  }

  get element() {
    return this.root;
  }
}
