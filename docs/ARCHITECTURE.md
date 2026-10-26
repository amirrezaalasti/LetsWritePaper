# Architecture

## One idea: an attributed, append-only log of CRDT updates

Each paper source file (`.tex`, `.typ`, `.bib`, …) is a Yjs text document (`yrs` in Rust,
`yjs` in the editor). Every change is stored as one row in `.paper/history.db` (SQLite, WAL),
whoever makes it:

```
updates(id, uid, doc, author_id, author_kind, author_name, session, origin, message, summary, added, removed, created_at, data)
```

- `author_kind` is `human`, `agent`, `external` (a file changed by another tool) or `system`.
- `message` holds the why: an agent's `rationale`, "accepted proposal #3", and similar.
- `summary` is a one-line description of the text that changed (`§ Method: "SGD" → "Adam"`).
  It is written for every edit, including ordinary typing, so the change log can show each one.
- `data` holds the Yjs v1 update bytes.

The same log serves four purposes:

1. **Sync between processes.** The app, MCP servers and CLI each open the project. They
   append their own updates and tail others' rows (`Workspace::poll`, triggered by
   `PRAGMA data_version`).
2. **History.** The change log lists every update, with `summary` saying what changed.
   The same rows can be grouped into edit sessions (`history::group_log`). Any past state
   can be rebuilt by replaying updates up to an id.
3. **Blame.** Every insert carries a formatting attribute `a = <author id>`, so authorship
   travels with the characters through merges. `PaperDoc::blame_runs` reads it back.
4. **Sync between devices.** See below.

## Processes and file ownership

Exactly one process per project is the **owner**: it holds the OS file lock on
`.paper/owner.lock`, which is released automatically on exit. The owner:

- mirrors each document to its file on disk, so compilers, git and editors see current text;
- watches the folder. When a file changes on disk, it waits briefly (agent hooks may claim
  the edit first), then records the change as an `external` edit. The edit is computed
  against what was last written (a "shadow" replica), so concurrent CRDT edits are never
  reverted;
- records what it last wrote (`.paper/mirror.json`: hash + history position), so the next
  owner can tell a file that is merely stale from one that was really edited while nobody
  was watching.

Non-owners (an MCP server started by Claude Code while the app is open, `paper` CLI commands)
write only to the log. When the owner exits, the next process to act takes ownership.

```
 Claude Code ──stdio/MCP──► paper mcp ──┐
 VS Code + hook ──► paper ingest ───────┤  append/tail
 Desktop app (owner) ◄──────────────────┼──► .paper/history.db ◄──► sync client ◄──HTTP──► relay ◄──► other devices
 paper watch / CLI ─────────────────────┘
        │ owner mirrors
        ▼
   main.tex, refs.bib …  ◄── other editors, git, latexmk
```

## Editor ↔ core

The editor keeps its own Yjs replica per open document:

- Local keystrokes become Yjs updates, tagged with `a`, batched for about 400 ms, and pushed
  to Rust (`push_update`). Each batch becomes one history row.
- Updates from elsewhere (agents, other devices, accepted proposals) arrive as
  `paper://doc-update` events and are applied with a "remote" origin, so they are not echoed
  back.
- Offsets are UTF-16 on both sides (`OffsetKind::Utf16`).

## Versions

- **Checkpoints and milestones** store a full CRDT snapshot of every document (in
  `checkpoint_docs`), so they reconstruct identically on every device. When the folder is a
  git repo they also create a commit, with `Co-Authored-By` trailers for each human and agent
  contributor.
- **Diffs** compare two versions word by word (`similar`) and group hunks by the section the
  change falls in (`outline.rs` parses LaTeX, Typst and Markdown headings).
- **Restore** writes old text as a new, attributed edit. History is never rewritten.

## Proposals and reviews

- A **proposal** is an `(old_text, new_text, rationale)` suggestion. Accepting it applies the
  edit credited to the proposer, noting who accepted. It is marked *stale* if the old text no
  longer exists.
- **Review rounds** belong to one reviewer (human or agent persona) and one version. Each
  comment anchors to a verbatim quote plus an offset hint. The comment's current position is
  resolved on read, or it is reported as rewritten.

## Skills

A skill is a `SKILL.md` (frontmatter `name, description, kind, sections`) in the Claude Code /
Agent Skills format. Sources are listed by precedence, highest first:

1. project `.paper/skills`
2. user config directory
3. `~/.claude/skills`
4. bundled

An override without `sections:` inherits the bundled skill's section globs. Skills that apply
to a section are, in order:

1. `[skills] global`
2. `[skills.sections]` globs in `paper.toml`
3. each skill's own `sections:` globs, which also match word-suffixes of the title, so
   `method*` matches "Our Method"

`section_context` hands agents the section text together with the full instructions of every
skill that applies and the open review comments on that section.

## Cross-device sync

Every shareable record has a global `uid`. Every local write appends `(kind, key, origin)` to
`changelog`.

- **Records.** `export_changes(after, exclude_origin)` yields the latest version of each
  changed record, skipping records whose latest change came from the receiving device.
  `import_records` applies records:
  - CRDT updates: insert-or-ignore by `uid`. They merge by construction.
  - Proposals, rounds, comments, document rows and shared files (`paper.toml`,
    `.paper/skills/**`): last-writer-wins by `updated_at`.
- **Relay** (`paper-sync/server.rs`). It keeps one store per shared paper and exposes three
  endpoints: `POST /v1/projects` (needs the relay secret if one is configured),
  `POST …/push` and `GET …/pull?after=&wait=` (long-poll). Every request after creation needs
  the paper's bearer token.
- **Client.** A push thread sends local changes every ~400 ms. A pull thread long-polls.
  Imported updates land in the local log with `origin = "sync"`, so every open `Workspace`
  picks them up through the same `poll` path as edits from another local process.

## Claude panel

The app spawns the user's `claude` CLI:

```
claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages \
  --mcp-config '{"mcpServers":{"paper":{"command":"<app>","args":["mcp","--agent","claude-code"]}}}' --strict-mcp-config \
  --allowedTools "mcp__paper__*,Read,Grep,Glob" --permission-prompt-tool stdio \
  --append-system-prompt "…" [--permission-mode acceptEdits] [--resume <id>]
```

- The app binary doubles as the MCP server (`letswritepaper mcp`), so a desktop install needs
  nothing else.
- Stream events (`system/init`, `stream_event` text deltas, `assistant` tool calls, `user`
  tool results, `result`) are forwarded to the panel. A tool that is not pre-approved arrives
  as a `control_request`; the panel shows Allow / Deny and writes the `control_response` back.
- Each message carries editor context (file, line, section, selection).
- Remote Control hand-off runs `claude --resume <id> --remote-control <title>` in a terminal.

ChatGPT and Gemini use the same panel and the same event shape. ChatGPT spawns
`codex exec --json` once per message (resumed by thread id) with the paper server passed
as `mcp_servers.paper`. Gemini keeps one `agy --input-format stream-json` process open and
writes the paper server into the workspace `.agents/mcp_config.json`. Their stdout is
translated into the Claude events above before it reaches the panel. A Claude message
can include image and PDF blocks. `/usage` and `/status` are answered from the signed-in
Claude account, because the headless session cannot open those screens. `/plugin` and the
other terminal-only commands are opened in Claude Code's terminal. `/context` and `/compact`
are sent through as typed. ChatGPT images are passed with `--image`.
Gemini only accepts text, so attached files are saved under `.paper/attachments` and named
in the prompt.
