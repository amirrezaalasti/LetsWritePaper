import { githubRepository, repositoryUrl } from "@/lib/site";

export function SiteFooter() {
  const repo = githubRepository();
  const source = repositoryUrl(repo);

  return (
    <footer className="border-t border-line">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-3 px-5 py-8 text-sm text-muted sm:flex-row sm:items-center sm:justify-between">
        <p>LetsWritePaper 0.1 · MIT</p>
        <p>
          macOS, Windows, and Linux
          {source ? (
            <>
              {" · "}
              <a href={source} className="underline decoration-line-strong underline-offset-4 hover:text-ink" rel="noopener noreferrer">
                {repo}
              </a>
            </>
          ) : null}
        </p>
      </div>
    </footer>
  );
}
