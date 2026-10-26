# LetsWritePaper

An editor for academic papers built around version control and AI co-authors. Humans and
agents (Claude Code, Claude in VS Code/Cursor, any MCP client) edit the same LaTeX or Typst
paper live. Every change is recorded with **who** made it, **when** and **why**, and can be
diffed by section, blamed line by line, checkpointed, restored, reviewed and synced to your
other devices.

- **Live co-editing.** Every document is a CRDT (Yjs-compatible, via `yrs`). The editor, agents,
  other tools writing files, and other devices all merge without conflicts.
- **Version control made for papers.** History is grouped into edit sessions per author.
  Diffs are word-level and grouped by section. Blame works per character. Named checkpoints
  and milestones (`submitted`, `rebuttal`, `camera-ready`, `arxiv`) also become git commits
  crediting every human and agent contributor.
- **AI agents as first-class co-authors.** A built-in MCP server offers 23 paper-aware tools:
  section context, exact-excerpt edits, suggested edits, compile, review and more. Agent
  edits show up live, attributed to the agent.
- **The Claude panel.** The app has its own Claude panel, like the VS Code/Cursor extension. It
  offers one-click Claude Code install, streaming chat that knows your file, section and
  selection, resumable sessions, and *Continue on another device* through Claude Code's
  Remote Control.
- **Skills.** A skill is a `SKILL.md` file of writing or review instructions, bound to sections
  (e.g. `intro*` → `paper-introduction`). 13 skills ship built in, your existing
  `~/.claude/skills` are picked up automatically, and you can add per-paper skills.
- **Peer review.** Review rounds can come from humans or reviewer personas (methods-skeptic,
  clarity, novelty, reproducibility, area chair). Comments carry severity and category and are
  anchored to quotes that follow the text as it changes. Rounds record rubric scores, take
  author responses, and export a Markdown response-to-reviewers letter.
- **Cross-device sync.** Share a paper from the app on your local network, or through a relay
  server (`paper serve`). Other devices join from a link and keep full, offline-capable
  copies.
- **Live preview.** Typst compiles in-process from the live text, typically in milliseconds.
  LaTeX compiles with `tectonic` (recommended) or `latexmk`, and diagnostics are clickable.
- **Cross-platform.** Written in Rust with a Tauri 2 desktop app for macOS, Windows and Linux,
  plus a `paper` CLI.

## Quick start

### Requirements

- Rust 1.89+ (`rustup`), Node 20+ and `pnpm`.
- For LaTeX preview: [`tectonic`](https://tectonic-typesetting.github.io/) or a TeX
  distribution with `latexmk`. Typst needs nothing extra.
- On Linux, the Tauri system packages: `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`.
- For the Claude panel: Claude Code (the app can install it for you) and a Claude account or API key.

### Desktop app

```sh
cd app
pnpm install
pnpm tauri dev          # development
pnpm tauri build        # installers for your platform (dmg/app, msi/exe, deb/rpm/AppImage)
```

### CLI

```sh
cargo install --path crates/paper-cli      # installs `paper`
paper init --title "My Paper" --format typst   # or: latex (imports existing .tex/.bib)
paper watch                                    # mirror files, record outside edits, sync
```

## Working with agents

`paper init` (and *New paper* in the app) configures the project for Claude Code:

- `.mcp.json` registers the `paper` MCP server.
- `.claude/settings.json` adds a hook that credits Claude's direct file edits to `claude-code`
  instead of "external".
- `AGENTS.md` / `CLAUDE.md` explain the workflow to agents.

Open the paper folder in VS Code or Cursor with the Claude extension, or run `claude` there,
and ask for what you need ("tighten the related work", "run a mock NeurIPS review"). For other
MCP clients, run `paper mcp` (or `letswritepaper mcp`) in the project folder. Agents identify
themselves through MCP; override with `--agent <name>`.

Prompts are exposed as slash commands (`/mcp__paper__write-section`, `/mcp__paper__review-paper`,
`/mcp__paper__address-reviews`, and one `skill:<name>` per skill).

<details>
<summary>MCP tools</summary>

`project_overview`, `read_document`, `outline`, `search`, `section_context`, `edit_document`,
`write_document`, `propose_edit`, `list_proposals`, `history`, `diff`, `blame`, `checkpoint`,
`compile`, `list_skills`, `get_skill`, `bind_skill`, `create_skill`, `start_review`,
`add_review_comment`, `finish_review`, `list_review_comments`, `respond_to_comment`.
</details>

### The Claude panel

Click **◆ Claude** (⌘/Ctrl+L) in the app. If Claude Code is missing, **Install** runs the
official installer (`curl -fsSL https://claude.ai/install.sh | bash`, or `install.ps1` on
Windows), and 🔑 opens a terminal to sign in. Chats run your own Claude Code
(`claude -p --input-format stream-json …`), connected only to this paper's MCP server.

- **Modes.** *Paper tools* (default) lets Claude work only through the paper tools and read
  files. *Paper tools + file edits* also allows its built-in editing tools.
- **Context.** Each message carries the open file, cursor section and selection (toggle
  *Editor context*).
- **Sessions.** Pick an earlier chat from the menu to resume it (`--resume`).
- **📱 Continue on another device.** Hands the chat to Claude Code's
  [Remote Control](https://code.claude.com/docs/en/remote-control.md), so you can keep talking
  to it from claude.ai/code or the Claude mobile app while it keeps editing the paper on this
  computer. This needs a Claude Pro, Max, Team or Enterprise login (API keys are not
  supported) and this computer must stay awake.

## Sharing across devices

In the app, go to **⇄ Share**:

- **Share on this network.** The app hosts a relay. Devices on the same Wi-Fi/LAN join while it
  runs.
- **Share via relay server.** Use a relay reachable from anywhere:

  ```sh
  paper serve --port 8787 --data /var/lib/paper-relay --secret "$(openssl rand -hex 16)"
  # or with Docker:
  docker build -f deploy/relay.Dockerfile -t paper-relay .
  docker run -p 8787:8787 -v paper-relay:/data -e PAPER_RELAY_SECRET=… paper-relay
  ```

  Put it behind HTTPS (Caddy, nginx, or a tunnel such as Cloudflare Tunnel or Tailscale).

On the other device, use **Join a shared paper…** in the app, or run
`paper join '<link>' [dir]` and then `paper watch`. Every copy keeps its full history, works
offline, and merges when it reconnects. The link contains the paper's access token, so
anyone who has it can read and edit the paper. Share it like a password.

## Skills

```sh
paper skill list                              # bundled, ~/.claude/skills, user and project skills
paper skill for "Related Work"                # which skills apply to a section, and why
paper skill bind paper-methods --section "approach*"
paper skill add ./my-venue-style              # folder containing SKILL.md (project) — or --user
paper skill new lab-style --section "intro*"  # scaffold .paper/skills/lab-style/SKILL.md
```

Skill bindings live in `paper.toml`:

```toml
[skills]
global = ["clarity-editing", "latex-hygiene"]

[skills.sections]
"abstract" = ["paper-abstract"]
"related*" = ["my-lab-related-work"]
```

## Version control

```sh
paper log                      # edit sessions: who (human/agent), what, why
paper diff draft-1 now         # word-level, grouped by section
paper blame main.tex
paper checkpoint submitted-v1 -m "NeurIPS submission" --milestone submitted
paper show main.tex --at submitted-v1
paper restore main.tex draft-1 # recorded as a new edit; nothing is lost
paper proposals                # suggested edits: accept / reject
paper review comments --status open
paper review letter > response.md
```

## Repository layout

| Path | What it is |
| --- | --- |
| `crates/paper-core` | Document CRDTs, history store, diff/blame, checkpoints, proposals, reviews, skills, sync records |
| `crates/paper-compile` | In-process Typst; LaTeX via tectonic/latexmk; diagnostics |
| `crates/paper-mcp` | MCP server (JSON-RPC over stdio) |
| `crates/paper-sync` | Relay server and sync client |
| `crates/paper-cli` | The `paper` command |
| `app/` | Tauri 2 desktop app (Rust backend in `src-tauri`, TypeScript + CodeMirror frontend) |
| `skills/` | Bundled skills (embedded at build time) |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## Status and known limitations

This is an early (0.1) version.

- No SyncTeX jump between the PDF and the source yet. LaTeX preview re-renders the whole PDF.
- The relay speaks plain HTTP. Use HTTPS in front of it for anything beyond a trusted LAN.
- The Claude panel drives the Claude Code CLI installed on your machine, with your own
  account. If you plan to distribute builds to other people, review Anthropic's terms on
  offering claude.ai login in third-party products; API-key login is the safe default.
- Tests run on macOS. Windows and Linux builds are covered by CI configuration but haven't
  been exercised by hand yet.

## Development

```sh
cargo test --workspace         # core, compile, MCP, sync (incl. a live relay test)
cargo clippy --workspace --all-targets
cd app && pnpm typecheck
```
