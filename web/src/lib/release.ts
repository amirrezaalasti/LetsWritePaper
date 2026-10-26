import { cacheLife, cacheTag } from "next/cache";

const REPO = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

interface GitHubRelease {
  tag_name?: string;
  html_url?: string;
  published_at?: string;
}

export type ReleaseResult =
  | { status: "ready"; version: string; publishedLabel: string; url: string }
  | { status: "unpublished" }
  | { status: "unconfigured" }
  | { status: "unavailable" };

function displayVersion(tag: string): string {
  return tag.replace(/^v(?=\d)/, "");
}

function formatReleaseDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  }).format(date);
}

export async function getLatestRelease(repo: string): Promise<ReleaseResult> {
  "use cache";
  cacheLife({
    stale: 60 * 5,
    revalidate: 60 * 15,
    expire: 60 * 60 * 24,
  });
  cacheTag("github-release");

  if (!REPO.test(repo)) return { status: "unconfigured" };

  try {
    const response = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "LetsWritePaper",
        "X-GitHub-Api-Version": "2022-11-28",
      },
      signal: AbortSignal.timeout(8000),
    });

    if (response.status === 404) return { status: "unpublished" };
    if (!response.ok) return { status: "unavailable" };

    const body = (await response.json()) as GitHubRelease;
    const tag = body.tag_name?.trim();
    const url = body.html_url?.trim();
    if (!tag || !url) return { status: "unpublished" };

    return {
      status: "ready",
      version: displayVersion(tag),
      publishedLabel: body.published_at ? formatReleaseDate(body.published_at) : "",
      url,
    };
  } catch {
    return { status: "unavailable" };
  }
}

export function releaseNote(release: ReleaseResult): string {
  if (release.status === "ready") {
    const when = release.publishedLabel ? ` · ${release.publishedLabel}` : "";
    return `Version ${release.version}${when}. These are the files on the latest GitHub release.`;
  }
  if (release.status === "unconfigured") {
    return "Download links need GITHUB_REPOSITORY set to owner/name.";
  }
  return "Those files are published when you push a version tag such as v0.1.0. Until that first release exists, the links have nothing to download.";
}
