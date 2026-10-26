// First-launch wizard: name for edit history, optional tectonic install, optional Claude Code.
import { api, events, type SetupStatus } from "./api";
import { clear, h, toast } from "./ui";

const STEPS = ["Name", "Writing", "Assistants", "Ready"] as const;

const app = document.getElementById("app")!;

export async function setupWizard(onDone: () => void): Promise<void> {
  let status = await api.setupStatus().catch(
    () =>
      ({
        name: "",
        setup_done: false,
        typst: true,
        latex: null,
        latex_path: null,
        claude_installed: false,
        claude_version: null,
        codex_installed: false,
        codex_version: null,
        gemini_installed: false,
        gemini_version: null,
        tectonic_command: "",
      }) satisfies SetupStatus,
  );
  let name = status.name;
  let step = 0;
  let busy = false;

  const persist = async (done: boolean) => {
    const trimmed = name.trim();
    if (!trimmed) {
      toast("Enter the name your edits should be attributed to", "error");
      return false;
    }
    try {
      status = await api.setupSave(trimmed, done);
      name = status.name;
      return true;
    } catch (e) {
      toast(String(e), "error");
      return false;
    }
  };

  const go = (next: number) => {
    if (busy) return;
    step = next;
    render();
  };

  const finish = async () => {
    if (!name.trim()) name = "author";
    if (await persist(true)) onDone();
  };

  function render() {
    const steps = h(
      "div",
      { class: "setup-steps" },
      ...STEPS.map((label, i) => h("span", { class: i === step ? "on" : "" }, `${i + 1} ${label}`)),
    );
    const body = h("div", {});
    const actions = h("div", { class: "actions" });

    if (step === 0) {
      const input = h("input", {
        type: "text",
        value: name,
        placeholder: "Ada Lovelace",
        "aria-label": "Your name",
      });
      input.addEventListener("input", () => {
        name = input.value;
        input.classList.remove("invalid");
      });
      const continueName = async () => {
        if (!name.trim()) {
          input.classList.add("invalid");
          input.focus();
          return;
        }
        if (await persist(status.setup_done)) go(1);
      };
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          void continueName();
        }
      });
      body.append(
        h("p", {}, "Edits you make are attributed to this name, in the history and in blame."),
        h("label", {}, h("span", { class: "muted" }, "Your name"), input),
      );
      actions.append(
        h("button", { class: "primary", onclick: () => void continueName() }, "Continue"),
        h("button", { class: "ghost", onclick: () => void finish() }, "Skip setup"),
      );
      queueMicrotask(() => input.focus());
    } else if (step === 1) {
      body.append(
        h("p", {}, "Typst preview is built into the app. LaTeX preview uses tectonic, one program that downloads the packages a paper needs."),
        toolList(status),
      );
      if (!status.latex && status.tectonic_command) {
        body.append(h("code", { class: "block" }, status.tectonic_command));
      }
      const log = h("pre", { class: "install-log" });
      body.append(log);
      const install = h("button", {
        class: "primary",
        onclick: async () => {
          busy = true;
          install.disabled = true;
          install.textContent = "Installing tectonic…";
          const unlisten = await events.onSetupLog((line) => {
            log.textContent += `${line}\n`;
            log.scrollTop = log.scrollHeight;
          });
          try {
            status = await api.installTectonic();
            toast(status.latex ? `Installed ${status.latex}` : "Tectonic install finished");
            busy = false;
            render();
          } catch (e) {
            toast(String(e), "error");
            busy = false;
            install.disabled = false;
            install.textContent = "Retry tectonic";
          } finally {
            unlisten();
          }
        },
      }, status.latex === "tectonic" ? "Reinstall tectonic" : "Install tectonic");
      const next = h("button", { class: status.latex === "tectonic" ? "primary" : "", onclick: () => go(2) }, "Continue");
      if (status.latex === "tectonic") install.classList.remove("primary");
      actions.append(status.latex === "tectonic" ? next : install, status.latex === "tectonic" ? install : next, h("button", { class: "ghost", onclick: () => go(0) }, "Back"));
    } else if (step === 2) {
      const tools: { id: string; product: string; installed: boolean; version: string | null }[] = [
        { id: "claude", product: "Claude Code", installed: status.claude_installed, version: status.claude_version },
        { id: "chatgpt", product: "Codex", installed: status.codex_installed, version: status.codex_version },
        { id: "gemini", product: "Antigravity", installed: status.gemini_installed, version: status.gemini_version },
      ];
      body.append(
        h("p", {}, "The assistant panel runs Claude, ChatGPT, or Gemini on this computer, each with your own account. Install the ones you want now. You can add the others later from the panel."),
        h(
          "div",
          { class: "setup-list" },
          ...tools.map((tool) => row(tool.installed, tool.installed ? `${tool.product} ${tool.version ?? ""}`.trim() : `${tool.product} is not installed yet`)),
        ),
      );
      const log = h("pre", { class: "install-log" });
      body.append(log);
      const any = tools.some((tool) => tool.installed);
      let primaryGiven = any;
      for (const tool of tools) {
        const install = h("button", {
          class: tool.installed || primaryGiven ? "" : "primary",
          onclick: async () => {
            busy = true;
            install.disabled = true;
            install.textContent = `Installing ${tool.product}…`;
            const unlisten = await events.onInstallLog((line) => {
              log.textContent += `${line}\n`;
              log.scrollTop = log.scrollHeight;
            });
            try {
              const installed = await api.claudeInstall(tool.id);
              status = await api.setupStatus();
              toast(installed.installed ? `${tool.product} ${installed.version ?? ""} installed` : `${tool.product} install finished`);
              busy = false;
              render();
            } catch (e) {
              toast(String(e), "error");
              busy = false;
              install.disabled = false;
              install.textContent = `Retry ${tool.product}`;
            } finally {
              unlisten();
            }
          },
        }, tool.installed ? `Reinstall ${tool.product}` : `Install ${tool.product}`);
        if (!tool.installed) primaryGiven = true;
        actions.append(install);
      }
      actions.append(h("button", { class: any ? "primary" : "", onclick: () => go(3) }, "Continue"), h("button", { class: "ghost", onclick: () => go(1) }, "Back"));
    } else {
      body.append(
        h("p", {}, "This computer is ready. You can open this wizard again from the start screen."),
        h(
          "div",
          { class: "setup-list" },
          row(true, `Edits are attributed to ${name.trim() || status.name}`),
          row(status.typst, status.typst ? "Typst preview is built in" : "This build does not include Typst"),
          row(true, status.latex ? `${status.latex} is available for LaTeX` : "LaTeX preview can be added later with tectonic"),
          row(status.claude_installed, status.claude_installed ? `Claude Code ${(status.claude_version ?? "").trim()}` : "Claude Code can be installed later from the assistant panel"),
          row(status.codex_installed, status.codex_installed ? `Codex ${(status.codex_version ?? "").trim()}` : "Codex can be installed later for ChatGPT"),
          row(status.gemini_installed, status.gemini_installed ? `Antigravity ${(status.gemini_version ?? "").trim()}` : "Antigravity can be installed later for Gemini"),
        ),
      );
      actions.append(
        h("button", { class: "primary", onclick: () => void finish() }, "Start writing"),
        h("button", { class: "ghost", onclick: () => go(2) }, "Back"),
      );
    }

    clear(
      app,
      h(
        "div",
        { class: "setup" },
        h("section", { class: "setup-card", "aria-label": "Setup" }, steps, h("h1", {}, STEPS[step]), body, actions),
      ),
    );
  }

  render();
}

function toolList(status: SetupStatus): HTMLElement {
  const latex = status.latex
    ? `${status.latex}${status.latex_path ? ` · ${status.latex_path}` : ""}`
    : "LaTeX engine is not installed yet";
  return h(
    "div",
    { class: "setup-list" },
    row(status.typst, status.typst ? "Typst preview is built in" : "This build does not include Typst"),
    row(!!status.latex, latex),
  );
}

function row(ok: boolean, text: string): HTMLElement {
  return h("div", { class: "setup-row" }, h("span", { class: `dot ${ok ? "ok" : "bad"}` }), h("span", {}, text));
}
