import { DownloadFiles } from "@/components/download-files";
import { downloadsFor } from "@/lib/downloads";
import { getLatestRelease, releaseNote } from "@/lib/release";
import { githubRepository } from "@/lib/site";

export async function InstallPanel() {
  const repo = githubRepository();
  const downloads = downloadsFor(repo);
  if (!downloads) {
    return (
      <p className="max-w-2xl leading-7 text-ink">
        Download links need GITHUB_REPOSITORY set to owner/name, the repository that publishes the installers.
      </p>
    );
  }

  const release = await getLatestRelease(repo);
  return (
    <DownloadFiles
      files={downloads.files}
      note={releaseNote(release)}
      releaseUrl={release.status === "ready" ? release.url : null}
    />
  );
}
