export function Manuscript() {
  return (
    <figure className="relative">
      <div
        aria-hidden
        className="absolute -left-3 top-8 hidden h-[88%] w-px bg-line sm:block"
      />
      <div className="rounded-sm border border-line bg-sheet px-6 py-6 shadow-[0_18px_50px_rgba(60,40,10,0.08)] sm:px-8">
        <figcaption className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b border-line pb-3 text-xs tracking-wide text-muted uppercase">
          <span>Example edit</span>
          <span className="font-mono normal-case tracking-normal">main.typ</span>
        </figcaption>
        <p className="mt-5 font-serif text-sm text-muted">Related work</p>
        <p className="mt-2 font-serif text-xl leading-snug text-balance text-ink">
          The history of a sentence is left in{" "}
          <span className="bg-[#fde8e6] text-[#9d2c2c] line-through decoration-[#9d2c2c]/70">a commit message</span>{" "}
          <span className="bg-[#e5f6ea] text-[#146c36]">the name of the person who wrote it</span>.
        </p>
        <p className="mt-6 flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-line pt-3 text-sm text-muted">
          <span className="font-medium text-agent">claude-code</span>
          <span>exact edit</span>
          <span>Related work</span>
        </p>
      </div>
    </figure>
  );
}
