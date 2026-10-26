"use client";

import { useSyncExternalStore } from "react";
import type { DownloadTarget } from "@/lib/downloads";

type Visitor = DownloadTarget["id"] | "mobile" | null;

function subscribe() {
  return () => {};
}

function detectVisitor(): Visitor {
  const ua = navigator.userAgent;
  const touchMac = /Macintosh/i.test(ua) && navigator.maxTouchPoints > 1;
  if (/Android|iPhone|iPad|iPod/i.test(ua) || touchMac) return "mobile";
  if (/Mac/i.test(ua)) return "macos";
  if (/Win/i.test(ua)) return "windows";
  if (/Linux/i.test(ua)) return "linux";
  return null;
}

function useVisitor(): Visitor {
  return useSyncExternalStore(subscribe, detectVisitor, () => null);
}

function AfterDownload({ file }: { file: DownloadTarget }) {
  if (file.id === "windows") {
    return <p className="mt-4 leading-7 text-muted">Run it. That setup wizard installs the app for the current user.</p>;
  }
  if (file.id === "linux") {
    return (
      <p className="mt-4 leading-7 text-muted">
        Or use the same <code className="font-mono text-sm text-ink">curl</code> command as on macOS.
      </p>
    );
  }
  return <p className="mt-4 leading-7 text-muted">Or run:</p>;
}

export function DownloadFiles({
  files,
  note,
  releaseUrl,
}: {
  files: DownloadTarget[];
  note: string;
  releaseUrl: string | null;
}) {
  const visitor = useVisitor();
  const order =
    visitor === "macos" || visitor === "windows" || visitor === "linux"
      ? [visitor, ...files.map((file) => file.id).filter((id) => id !== visitor)]
      : files.map((file) => file.id);

  return (
    <div>
      {visitor === "mobile" ? (
        <p className="mb-4 max-w-2xl text-sm leading-6 text-ink">
          LetsWritePaper runs on a Mac, Windows PC, or Linux computer. Open this page there to install it. The files below are the same ones.
        </p>
      ) : null}
      <ul className="grid gap-4 lg:grid-cols-3">
        {order.map((id) => {
          const file = files.find((item) => item.id === id);
          if (!file) return null;
          const current = visitor === id;
          return (
            <li key={file.id}>
              <article
                className={`flex h-full flex-col rounded-sm border bg-sheet p-5 ${
                  current ? "border-accent shadow-[0_0_0_1px_var(--accent)]" : "border-line"
                }`}
              >
                <p className="text-xs tracking-wide text-muted uppercase">{current ? "This computer" : "Desktop"}</p>
                <h3 className="mt-2 font-serif text-3xl tracking-tight">{file.label}</h3>
                <a
                  href={file.url}
                  className="mt-5 inline-flex flex-col rounded-full bg-accent px-4 py-3 text-white hover:bg-accent-ink"
                >
                  <span className="font-medium">Download</span>
                  <span className="text-sm break-all text-white/80">{file.name}</span>
                </a>
                <AfterDownload file={file} />
                {file.command ? (
                  <pre className="mt-3 overflow-x-auto font-mono text-sm whitespace-pre-wrap text-ink">{file.command}</pre>
                ) : null}
              </article>
            </li>
          );
        })}
      </ul>
      <p className="mt-6 max-w-2xl leading-7 text-muted">
        {note}
        {releaseUrl ? (
          <>
            {" "}
            <a href={releaseUrl} className="text-accent-ink underline decoration-line underline-offset-4 hover:text-ink" rel="noopener noreferrer">
              Release notes
            </a>
          </>
        ) : null}
      </p>
    </div>
  );
}
