import Image, { type StaticImageData } from "next/image";
import Link from "next/link";
import { AppDemo } from "@/components/app-demo";
import { InstallPanel } from "@/components/install-panel";
import { Manuscript } from "@/components/manuscript";
import { maintainer } from "@/lib/site";
import coauthor from "@/assets/coauthor.webp";
import history from "@/assets/history.webp";
import review from "@/assets/review.webp";

const features = [
  {
    number: "01",
    title: "Live co-editing",
    body: "Every document is a CRDT. The editor, agents, other tools writing the files, and your other devices merge without a conflict to resolve.",
  },
  {
    number: "02",
    title: "A changelog for the paper",
    body: "Each edit is its own entry: who made it, when, and a short description. Diffs are word-level and grouped by section. Blame works per character. A restore is recorded as a new edit, so nothing is thrown away.",
  },
  {
    number: "03",
    title: "Checkpoints",
    body: "Name a moment: submitted, rebuttal, camera-ready, or arxiv. That checkpoint is also a git commit, and it credits every human and agent who contributed.",
  },
  {
    number: "04",
    title: "Agents as co-authors",
    body: "A built-in MCP server offers 23 paper-aware tools, from section context and exact edits to compile and review. Claude Code, Claude in VS Code or Cursor, and any other MCP client can use them. The edit shows up live, under the agent’s name.",
  },
  {
    number: "05",
    title: "Skills for a section",
    body: "A skill is a SKILL.md file of writing or review instructions, bound to sections. Thirteen ship with the app, skills you already keep for Claude are picked up, and a paper can carry its own.",
  },
  {
    number: "06",
    title: "Review that stays put",
    body: "Rounds can come from you or from a reviewer persona. Comments carry a severity and a category, and they stay anchored to the quoted text as it changes. A round exports as a Markdown response-to-reviewers letter.",
  },
];

const steps = [
  {
    title: "Someone edits",
    body: "You, in the editor, or an agent through the paper tools or the Claude panel. The panel already knows the open file, the section, and the selection.",
  },
  {
    title: "The change is stored",
    body: "The entry keeps the author, the time, and a short description. A stretch of edits by the same author can be read as one session.",
  },
  {
    title: "You can look back",
    body: "Diff two moments by section, blame a line down to the character, or open the file as it was at a checkpoint.",
  },
  {
    title: "You mark the moment",
    body: "A named checkpoint becomes a git commit. The people and agents who wrote the paper are on that commit.",
  },
];

function Illustration({ src, alt }: { src: StaticImageData; alt: string }) {
  return (
    <div className="overflow-hidden rounded-sm border border-line bg-sheet shadow-[0_18px_50px_rgba(60,40,10,0.06)]">
      <Image
        src={src}
        alt={alt}
        placeholder="blur"
        sizes="(min-width: 1024px) 34rem, 100vw"
        className="h-auto w-full"
      />
    </div>
  );
}

export default function HomePage() {
  return (
    <main>
      <section className="mx-auto grid w-full max-w-6xl items-center gap-12 px-5 py-16 lg:grid-cols-[1.15fr_0.85fr] lg:py-24">
        <div>
          <p className="text-sm tracking-wide text-muted uppercase">Version 0.1 · LaTeX and Typst</p>
          <h1 className="mt-4 max-w-xl font-serif text-5xl leading-[1.05] tracking-tight text-ink sm:text-6xl">
            Every edit keeps its <em>author.</em>
          </h1>
          <p className="mt-6 max-w-xl text-lg leading-8 text-muted">
            LetsWritePaper is an editor for academic papers. Humans and agents work on the same document. The app
            records who changed it, when, and why, then lets you diff a section, review it, and continue on another
            computer.
          </p>
          <div className="mt-8 flex flex-col gap-3 sm:flex-row">
            <Link
              href="/download"
              className="inline-flex h-12 items-center justify-center rounded-full bg-accent px-6 font-medium text-white hover:bg-accent-ink"
            >
              Download the app
            </Link>
            <a
              href="#demo"
              className="inline-flex h-12 items-center justify-center rounded-full border border-line-strong bg-sheet px-6 font-medium hover:border-ink"
            >
              See it work
            </a>
          </div>
        </div>
        <Manuscript />
      </section>

      <AppDemo />

      <section id="product" className="border-t border-line">
        <div className="mx-auto w-full max-w-6xl px-5 py-16 lg:py-24">
          <div className="grid items-center gap-10 lg:grid-cols-2">
            <div>
              <h2 className="font-serif text-4xl tracking-tight">What the app is for</h2>
              <p className="mt-4 max-w-2xl text-lg leading-8 text-muted">
                The desktop app is what you install. The <span className="font-mono text-base text-ink">paper</span>{" "}
                command does the same work from a terminal. Typst preview is built in. LaTeX preview uses tectonic, or
                latexmk if you already have a TeX distribution.
              </p>
            </div>
            <Illustration
              src={coauthor}
              alt="A hand with a pen and an AI agent edit the same manuscript page; the human's lines are marked in blue and the agent's in violet."
            />
          </div>
          <ol className="mt-12 grid gap-x-12 gap-y-10 md:grid-cols-2">
            {features.map((feature) => (
              <li key={feature.number}>
                <p className="font-mono text-xs text-muted">{feature.number}</p>
                <h3 className="mt-2 font-serif text-2xl tracking-tight">{feature.title}</h3>
                <p className="mt-2 max-w-prose leading-7 text-muted">{feature.body}</p>
              </li>
            ))}
          </ol>
        </div>
      </section>

      <section className="border-t border-line bg-sheet/60">
        <div className="mx-auto w-full max-w-6xl px-5 py-16 lg:py-24">
          <h2 className="font-serif text-4xl tracking-tight">What happens to an edit</h2>
          <div className="mt-10 grid items-start gap-10 lg:grid-cols-2">
            <ol className="grid gap-8 sm:grid-cols-2 lg:grid-cols-1">
              {steps.map((step, index) => (
                <li key={step.title} className="border-t border-line pt-4">
                  <p className="font-mono text-xs text-muted">0{index + 1}</p>
                  <h3 className="mt-2 font-serif text-2xl tracking-tight">{step.title}</h3>
                  <p className="mt-2 leading-7 text-muted">{step.body}</p>
                </li>
              ))}
            </ol>
            <div className="lg:sticky lg:top-24">
              <Illustration
                src={history}
                alt="A timeline of edits, blue for people and violet for agents, with a milestone flag, beside a page showing a word-level diff."
              />
            </div>
          </div>
          <div className="mt-12 grid items-center gap-10 border-t border-line pt-8 lg:grid-cols-2">
            <Illustration
              src={review}
              alt="The same manuscript with anchored review comments open on a laptop, a second laptop, and a tablet, linked to show sync."
            />
            <div className="grid gap-8">
              <div>
                <h3 className="font-serif text-2xl tracking-tight">Preview</h3>
                <p className="mt-2 leading-7 text-muted">
                  Typst compiles in the app, usually in milliseconds. LaTeX compiles with tectonic or latexmk, and the
                  diagnostics are clickable. The first launch can install tectonic for you.
                </p>
              </div>
              <div>
                <h3 className="font-serif text-2xl tracking-tight">Another computer</h3>
                <p className="mt-2 leading-7 text-muted">
                  Share a paper on the local network, or through a relay. The other device joins from a link and keeps
                  a full copy that works offline, then merges when it reconnects. The link is the access token, so treat
                  it like a password.
                </p>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section id="download" className="border-t border-line">
        <div className="mx-auto w-full max-w-6xl px-5 py-16 lg:py-24">
          <h2 className="font-serif text-4xl tracking-tight">Download</h2>
          <p className="mt-4 max-w-2xl text-lg leading-8 text-muted">
            People install LetsWritePaper from the GitHub release. Cloning the repository is only for building it
            yourself.
          </p>
          <div className="mt-8">
            <InstallPanel />
          </div>
          <p className="mt-8 text-sm">
            <Link href="/download" className="text-accent-ink underline decoration-line underline-offset-4 hover:text-ink">
              First launch, and building it yourself
            </Link>
          </p>
        </div>
      </section>

      <section id="contact" className="border-t border-line bg-sheet/60">
        <div className="mx-auto grid w-full max-w-6xl gap-10 px-5 py-16 lg:grid-cols-2 lg:py-24">
          <div>
            <h2 className="font-serif text-4xl tracking-tight">Investment and development</h2>
            <p className="mt-4 max-w-xl text-lg leading-8 text-muted">
              LetsWritePaper is built by {maintainer.name}. To invest, fund a feature, or build something on top of the
              app, write to me directly.
            </p>
          </div>
          <dl className="grid content-start gap-5 text-base">
            {[
              { label: "Email", href: `mailto:${maintainer.email}`, text: maintainer.email },
              { label: "Website", href: maintainer.website, text: "amirrezaalasti.de" },
              { label: "LinkedIn", href: maintainer.linkedin, text: "Amirreza Alasti" },
              { label: "GitHub", href: maintainer.github, text: "amirrezaalasti" },
            ].map((item) => (
              <div key={item.label} className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1 border-t border-line pt-4">
                <dt className="font-mono text-xs tracking-wide text-muted uppercase">{item.label}</dt>
                <dd>
                  <a
                    href={item.href}
                    className="break-all text-accent-ink underline decoration-line underline-offset-4 hover:text-ink"
                    rel={item.href.startsWith("http") ? "noopener noreferrer" : undefined}
                  >
                    {item.text}
                  </a>
                </dd>
              </div>
            ))}
          </dl>
        </div>
      </section>
    </main>
  );
}
