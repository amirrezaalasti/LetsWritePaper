import Image from "next/image";

const clips = [
  {
    src: "/media/agent-live.gif",
    width: 1100,
    height: 678,
    title: "An agent edits, and you watch it happen",
    body: "Claude Code edits the paper through the built-in MCP server. The text changes in the editor, the change log adds an entry with the agent's reason, and the preview recompiles.",
    alt: "Claude Code edits the Method section through MCP; the editor, change log, and Typst preview update live.",
  },
  {
    src: "/media/typing.gif",
    width: 1000,
    height: 827,
    title: "Typst preview while you type",
    body: "Typst compiles inside the app from the live text, usually in well under a second, so the page on the right follows every sentence.",
    alt: "Typing a sentence into the Conclusion while the compiled page on the right updates.",
  },
  {
    src: "/media/accept-suggestion.gif",
    width: 1100,
    height: 678,
    title: "Suggestions you can accept",
    body: "An agent can propose an edit instead of making it. Accepting applies the change under the agent's name, and the reviewer comment it answers stays anchored to the text.",
    alt: "Accepting a suggested edit in the review panel; the Experiments paragraph changes and the preview recompiles.",
  },
];

const shots = [
  { src: "/media/editor.webp", title: "Editor and section skills", body: "The outline lists the skills agents follow in each section." },
  { src: "/media/history.webp", title: "Change log", body: "Every edit, with who made it, what changed, and why." },
  { src: "/media/blame.webp", title: "Blame", body: "Who wrote each line, people and agents alike." },
  { src: "/media/diff.webp", title: "Diff by section", body: "Word-level changes between a checkpoint and now." },
  { src: "/media/review.webp", title: "Peer review", body: "Reviewer comments with severity, plus suggested edits." },
  { src: "/media/skills.webp", title: "Skills", body: "Bundled skills and the ones you already keep for Claude." },
];

export function AppDemo() {
  return (
    <section id="demo" className="border-t border-line">
      <div className="mx-auto w-full max-w-6xl px-5 py-16 lg:py-24">
        <h2 className="font-serif text-4xl tracking-tight">See it work</h2>
        <p className="mt-4 max-w-2xl text-lg leading-8 text-muted">
          Recordings of the desktop app on a sample Typst paper. The agent edits were made through the same MCP tools
          Claude Code uses.
        </p>

        <ol className="mt-12 grid gap-16">
          {clips.map((clip, index) => (
            <li
              key={clip.src}
              className={`grid items-center gap-8 ${index % 2 ? "lg:grid-cols-[1.6fr_0.8fr]" : "lg:grid-cols-[0.8fr_1.6fr]"}`}
            >
              <div className={index % 2 ? "lg:order-2" : undefined}>
                <p className="font-mono text-xs text-muted">0{index + 1}</p>
                <h3 className="mt-2 font-serif text-2xl tracking-tight">{clip.title}</h3>
                <p className="mt-2 leading-7 text-muted">{clip.body}</p>
              </div>
              <div className="overflow-hidden rounded-md border border-line-strong bg-ink shadow-[0_18px_50px_rgba(60,40,10,0.12)]">
                <Image
                  src={clip.src}
                  alt={clip.alt}
                  width={clip.width}
                  height={clip.height}
                  unoptimized
                  className="h-auto w-full"
                />
              </div>
            </li>
          ))}
        </ol>

        <h3 className="mt-20 font-serif text-3xl tracking-tight">Inside the app</h3>
        <ul className="mt-8 grid gap-x-8 gap-y-10 md:grid-cols-2 lg:grid-cols-3">
          {shots.map((shot) => (
            <li key={shot.src}>
              <a
                href={shot.src}
                className="block overflow-hidden rounded-md border border-line-strong bg-ink transition hover:border-ink"
                aria-label={`${shot.title}, full size`}
              >
                <Image
                  src={shot.src}
                  alt={`${shot.title} in LetsWritePaper`}
                  width={1600}
                  height={986}
                  sizes="(min-width: 1024px) 22rem, (min-width: 768px) 50vw, 100vw"
                  className="h-auto w-full"
                />
              </a>
              <p className="mt-3 font-medium">{shot.title}</p>
              <p className="mt-1 text-sm leading-6 text-muted">{shot.body}</p>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
