import type { Metadata } from "next";
import { InstallPanel } from "@/components/install-panel";
import { githubRepository, repositoryUrl } from "@/lib/site";

export const metadata: Metadata = {
  title: "Download",
  description: "Install LetsWritePaper for macOS, Windows, and Linux from the GitHub release.",
};

export default function DownloadPage() {
  const source = repositoryUrl(githubRepository());

  return (
    <main className="mx-auto w-full max-w-6xl px-5 py-16 lg:py-24">
      <p className="text-sm tracking-wide text-muted uppercase">Desktop app</p>
      <h1 className="mt-4 max-w-3xl font-serif text-5xl tracking-tight">Download LetsWritePaper</h1>
      <p className="mt-6 max-w-2xl text-lg leading-8 text-muted">
        People install LetsWritePaper from the GitHub release. Cloning the repository is only for building it yourself.
      </p>

      <div className="mt-10">
        <InstallPanel />
      </div>

      <section className="mt-16 border-t border-line pt-12">
        <h2 className="font-serif text-3xl tracking-tight">The first time it opens</h2>
        <p className="mt-4 max-w-2xl leading-7 text-muted">
          The first time the app opens, its setup wizard asks for your name and can install tectonic and Claude Code.
        </p>
      </section>

      <section id="build" className="mt-16 border-t border-line pt-12">
        <h2 className="font-serif text-3xl tracking-tight">Build it yourself</h2>
        <p className="mt-4 max-w-2xl leading-7 text-muted">
          A clone is for this, not for installing the app. You need Rust 1.95 or newer, Node 20 or newer, and pnpm. The
          command-line tool is installed from the same checkout.
        </p>
        <pre className="mt-6 max-w-full overflow-x-auto rounded-sm border border-line bg-sheet p-5 font-mono text-sm leading-7 whitespace-pre-wrap text-ink">{`cd app
pnpm install
pnpm tauri build

cargo install --path crates/paper-cli
paper init --title "My Paper" --format typst`}</pre>
        {source ? (
          <p className="mt-4 text-sm">
            <a href={source} className="text-accent-ink underline decoration-line underline-offset-4 hover:text-ink" rel="noopener noreferrer">
              Repository
            </a>
          </p>
        ) : null}
      </section>
    </main>
  );
}
