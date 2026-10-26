// Small DOM helpers: element builder, modal forms, toasts.

type Child = Node | string | null | undefined | false;
type Attrs = Record<string, string | number | boolean | EventListener | undefined>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === "class") {
      el.className = String(v);
    } else if (v === true) {
      el.setAttribute(k, "");
    } else {
      el.setAttribute(k, String(v));
    }
  }
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return el;
}

export function clear(el: HTMLElement, ...children: Child[]) {
  el.replaceChildren();
  for (const c of children) {
    if (c) el.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
}

export function timeAgo(ms: number): string {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return new Date(ms).toLocaleDateString();
}

export function authorBadge(a: { name: string; kind: string }): HTMLElement {
  const icon = a.kind === "agent" ? "◆" : a.kind === "human" ? "●" : "○";
  return h("span", { class: `badge author-${a.kind}`, title: `${a.name} (${a.kind})` }, `${icon} ${a.name}`);
}

export interface Field {
  name: string;
  label: string;
  type?: "text" | "textarea" | "select" | "number";
  value?: string;
  options?: [string, string][];
  placeholder?: string;
  required?: boolean;
}

/** Show a modal form; resolves to the values, or null when cancelled. */
export function form(title: string, fields: Field[], submitLabel = "OK", intro?: string): Promise<Record<string, string> | null> {
  return new Promise((resolve) => {
    const inputs: Record<string, HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement> = {};
    const body = h("div", { class: "form" });
    if (intro) body.append(h("p", { class: "muted" }, intro));
    for (const f of fields) {
      let input: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;
      if (f.type === "textarea") {
        input = h("textarea", { rows: 6, placeholder: f.placeholder ?? "" });
      } else if (f.type === "select") {
        input = h("select", {});
        for (const [v, label] of f.options ?? []) input.append(h("option", { value: v }, label));
      } else {
        input = h("input", { type: f.type ?? "text", placeholder: f.placeholder ?? "" });
      }
      if (f.value !== undefined) input.value = f.value;
      inputs[f.name] = input;
      body.append(h("label", {}, h("span", {}, f.label), input));
    }
    const close = (v: Record<string, string> | null) => {
      overlay.remove();
      resolve(v);
    };
    const submit = () => {
      const out: Record<string, string> = {};
      for (const f of fields) {
        out[f.name] = inputs[f.name].value;
        if (f.required && !out[f.name].trim()) {
          inputs[f.name].focus();
          inputs[f.name].classList.add("invalid");
          return;
        }
      }
      close(out);
    };
    const overlay = h(
      "div",
      { class: "overlay", onmousedown: (e: Event) => e.target === overlay && close(null) },
      h(
        "div",
        { class: "modal", role: "dialog", "aria-label": title },
        h("h2", {}, title),
        body,
        h(
          "div",
          { class: "modal-actions" },
          h("button", { class: "ghost", onclick: () => close(null) }, "Cancel"),
          h("button", { class: "primary", onclick: submit }, submitLabel),
        ),
      ),
    );
    overlay.addEventListener("keydown", (e) => {
      if (e.key === "Escape") close(null);
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
    });
    document.body.append(overlay);
    (Object.values(inputs)[0] as HTMLElement | undefined)?.focus();
  });
}

/** Read-only modal for long content (diffs, skill bodies, letters). */
export function showModal(title: string, content: Node, actions: [string, () => void][] = []) {
  const overlay = h("div", { class: "overlay", onmousedown: (e: Event) => e.target === overlay && overlay.remove() });
  const close = () => overlay.remove();
  overlay.append(
    h(
      "div",
      { class: "modal wide", role: "dialog", "aria-label": title },
      h("h2", {}, title),
      h("div", { class: "modal-scroll" }, content),
      h(
        "div",
        { class: "modal-actions" },
        ...actions.map(([label, fn]) => h("button", { onclick: () => (fn(), close()) }, label)),
        h("button", { class: "primary", onclick: close }, "Close"),
      ),
    ),
  );
  overlay.addEventListener("keydown", (e) => e.key === "Escape" && close());
  document.body.append(overlay);
  (overlay.querySelector("button.primary") as HTMLElement).focus();
}

let toastHost: HTMLElement | null = null;
export function toast(message: string, kind: "info" | "error" = "info") {
  if (!toastHost) {
    toastHost = h("div", { class: "toasts", "aria-live": "polite" });
    document.body.append(toastHost);
  }
  const t = h("div", { class: `toast ${kind}` }, message);
  toastHost.append(t);
  setTimeout(() => t.remove(), kind === "error" ? 7000 : 3500);
}

export async function guard<T>(p: Promise<T>): Promise<T | undefined> {
  try {
    return await p;
  } catch (e) {
    toast(String(e), "error");
    return undefined;
  }
}
