import Image from "next/image";
import Link from "next/link";
import { githubRepository, repositoryUrl } from "@/lib/site";

export function SiteHeader() {
  const source = repositoryUrl(githubRepository());

  return (
    <header className="sticky top-0 z-20 border-b border-line/80 bg-paper/90 backdrop-blur-md">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-3 px-5 py-3 sm:flex-row sm:items-center sm:justify-between">
        <Link href="/" className="flex items-center gap-2.5 font-medium tracking-tight">
          <Image src="/icon.png" alt="" width={28} height={28} priority />
          LetsWritePaper
        </Link>
        <nav aria-label="Site" className="flex items-center gap-5 text-sm">
          <Link href="/#product" className="text-muted hover:text-ink">
            Product
          </Link>
          <Link href="/download" className="text-muted hover:text-ink">
            Download
          </Link>
          {source ? (
            <a href={source} className="text-muted hover:text-ink" rel="noopener noreferrer">
              Source
            </a>
          ) : null}
        </nav>
      </div>
    </header>
  );
}
