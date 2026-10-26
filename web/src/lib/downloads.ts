const REPO = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export interface DownloadTarget {
  id: "macos" | "windows" | "linux";
  label: string;
  name: string;
  url: string;
  command: string | null;
}

export interface Downloads {
  files: DownloadTarget[];
}

export function downloadsFor(repo: string): Downloads | null {
  if (!REPO.test(repo)) return null;
  const file = (name: string) => `https://github.com/${repo}/releases/latest/download/${name}`;
  const command = `curl -fsSL https://raw.githubusercontent.com/${repo}/main/install.sh | bash`;
  return {
    files: [
      {
        id: "macos",
        label: "macOS",
        name: "LetsWritePaper-macos.dmg",
        url: file("LetsWritePaper-macos.dmg"),
        command,
      },
      {
        id: "windows",
        label: "Windows",
        name: "LetsWritePaper-x64-setup.exe",
        url: file("LetsWritePaper-x64-setup.exe"),
        command: null,
      },
      {
        id: "linux",
        label: "Linux",
        name: "LetsWritePaper-x64.AppImage",
        url: file("LetsWritePaper-x64.AppImage"),
        command,
      },
    ],
  };
}
