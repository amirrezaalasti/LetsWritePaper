/** GitHub repository that publishes desktop installers, as `owner/name`. */
export function githubRepository(): string {
  const fromEnv = process.env.GITHUB_REPOSITORY?.trim();
  return fromEnv || "amirrezaalasti/LetsWritePaper";
}

const REPO = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export function repositoryUrl(repo = githubRepository()): string | null {
  if (!REPO.test(repo)) return null;
  return `https://github.com/${repo}`;
}

export function siteUrl(): URL {
  const explicit = process.env.NEXT_PUBLIC_SITE_URL?.trim();
  if (explicit) return new URL(explicit);
  const production = process.env.VERCEL_PROJECT_PRODUCTION_URL?.trim();
  if (production) return new URL(`https://${production}`);
  const deployment = process.env.VERCEL_URL?.trim();
  if (deployment) return new URL(`https://${deployment}`);
  return new URL("http://localhost:3000");
}

export const description =
  "An editor for academic papers. Humans and agents edit the same LaTeX or Typst paper, and every change keeps who made it, when, and why.";
