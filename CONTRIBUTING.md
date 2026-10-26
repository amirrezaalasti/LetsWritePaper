# Contributing to LetsWritePaper

Thanks for helping with the editor. This file is how to set up, what a change should include, and how your contribution is licensed.

## License of contributions

LetsWritePaper is licensed under the [MIT license](LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in LetsWritePaper is licensed under the MIT license, without any additional terms or conditions.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

## What to work on

Issues labeled for newcomers are a good start. A bug report or a short design note is welcome before a large change.

Please open a public issue for bugs and features. Report security problems the way [SECURITY.md](SECURITY.md) describes, rather than in a public issue.

## Setup

- Rust 1.95+ (`rustup`), Node 20+, and `pnpm`.
- On Linux, the Tauri packages: `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`.
- LaTeX preview uses `tectonic` or `latexmk`. Typst preview is built into the desktop app.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd app && pnpm install && pnpm typecheck
cd app && pnpm tauri dev
```

CI runs those checks on macOS, Windows, and Linux. `cargo test` needs `PAPER_NO_CLAUDE_SKILLS=1` when your machine should not load `~/.claude/skills`.

## Changes

- Keep a pull request focused on one change.
- Use a conventional commit title: `feat`, `fix`, `docs`, `refactor`, `test`, or `chore`.
- Explain why the change is needed in the pull request.
- Add or update a test when the behavior is in the Rust crates.
- Run the checks above before asking for review.
- Cite bibliography keys that already exist in the project. When a citation is missing, write `TODO` instead of inventing one.
- Do not commit secrets, `.env` files, or local paper history.

The desktop UI lives in `app/`. Document history, sync, skills, and review live in `crates/`. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) describes how they fit together.

## Releases

A tag named `v*` (for example `v0.1.0`) builds the desktop installers and publishes them on the GitHub release, including `LetsWritePaper-x64-setup.exe`.
