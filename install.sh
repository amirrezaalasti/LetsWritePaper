#!/usr/bin/env bash
# Install LetsWritePaper for the current user.
#
# Downloads the latest GitHub release and installs it for the current user.
# No clone is required:
#   curl -fsSL https://raw.githubusercontent.com/amirrezaalasti/LetsWritePaper/main/install.sh | bash
# Set LWP_GITHUB=owner/LetsWritePaper to use a different repository.
#
# The app then opens its own setup wizard for your name, tectonic, and Claude Code.
set -euo pipefail

# Releases are published from this repository when a v* tag is pushed.
DEFAULT_REPO="${LWP_GITHUB:-amirrezaalasti/LetsWritePaper}"

say() { printf '%s\n' "$*" >&2; }

# A piped script has no terminal on stdin. Install without questions in that case.
choose() {
  local prompt="$1" default="$2" reply
  if [[ ! -t 0 ]]; then
    printf '%s\n' "$default"
    return
  fi
  read -r -p "$prompt [$default]: " reply </dev/tty || true
  printf '%s\n' "${reply:-$default}"
}

is_yes() {
  case "$1" in
    [Yy]|[Yy][Ee][Ss]) return 0 ;;
    *) return 1 ;;
  esac
}

repo_from_url() {
  local url="${1%.git}"
  case "$url" in
    git@github.com:*) printf '%s\n' "${url#git@github.com:}" ;;
    https://github.com/*) printf '%s\n' "${url#https://github.com/}" ;;
    ssh://git@github.com/*) printf '%s\n' "${url#ssh://git@github.com/}" ;;
    *) printf '\n' ;;
  esac
}

lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }

kind_score() {
  local name
  name="$(lower "$1")"
  case "$(uname -s)" in
    Darwin)
      case "$name" in
        *.app) echo 5 ;;
        *.dmg) echo 4 ;;
        *) echo 0 ;;
      esac
      ;;
    Linux)
      case "$name" in
        *.appimage) echo 5 ;;
        *.deb) echo 4 ;;
        *.rpm) echo 3 ;;
        *) echo 0 ;;
      esac
      ;;
    *) echo 0 ;;
  esac
}

arch_score() {
  local name wrong=0 right=0
  name="$(lower "$1")"
  case "$(uname -m)" in
    arm64|aarch64)
      case "$name" in *x86_64*|*amd64*|*x64*) wrong=1 ;; esac
      case "$name" in *arm64*|*aarch64*|*universal*) right=1 ;; esac
      ;;
    *)
      case "$name" in *arm64*|*aarch64*) wrong=1 ;; esac
      case "$name" in *x86_64*|*amd64*|*x64*|*universal*) right=1 ;; esac
      ;;
  esac
  if [[ "$wrong" -eq 1 && "$right" -eq 0 ]]; then
    echo 0
  elif [[ "$right" -eq 1 ]]; then
    case "$name" in
      *universal*) echo 2 ;;
      *) echo 3 ;;
    esac
  else
    echo 2
  fi
}

best_scored() {
  local best="" best_score=0 item ks as total
  for item in "$@"; do
    [[ -z "$item" ]] && continue
    ks="$(kind_score "$item")"
    as="$(arch_score "$item")"
    if [[ "$ks" -eq 0 || "$as" -eq 0 ]]; then
      continue
    fi
    total=$((ks * 10 + as))
    if [[ "$total" -gt "$best_score" ]]; then
      best="$item"
      best_score="$total"
    fi
  done
  printf '%s\n' "$best"
}

is_user_installer() {
  local name
  name="$(lower "$1")"
  case "$(uname -s)" in
    Darwin) case "$name" in *.app|*.dmg) return 0 ;; esac ;;
    Linux) case "$name" in *.appimage) return 0 ;; esac ;;
  esac
  return 1
}

is_pkg_installer() {
  local name
  name="$(lower "$1")"
  case "$(uname -s)" in
    Linux) case "$name" in *.deb|*.rpm) return 0 ;; esac ;;
  esac
  return 1
}

split_installers() {
  user_items=()
  pkg_items=()
  local item
  for item in "$@"; do
    if is_user_installer "$item"; then
      user_items+=("$item")
    elif is_pkg_installer "$item"; then
      pkg_items+=("$item")
    fi
  done
}

release_urls() {
  local repo="$1" body
  if ! body="$(curl -fsSL -H "User-Agent: LetsWritePaper-installer" -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/${repo}/releases/latest")"; then
    say "No published release for ${repo}. Draft releases stay hidden until you publish them."
    return 0
  fi
  printf '%s\n' "$body" | grep -oE 'https://[^"[:space:]]+' || true
}

installed_path=""

install_macos_app() {
  local src="$1" dest
  dest="${HOME}/Applications/$(basename "$src")"
  mkdir -p "${HOME}/Applications"
  rm -rf "$dest"
  cp -R "$src" "$dest"
  if command -v xattr >/dev/null 2>&1; then
    xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true
  fi
  installed_path="$dest"
  say "Installed ${dest}"
}

install_dmg() {
  local dmg="$1" mnt app
  mnt="$(mktemp -d)"
  hdiutil attach "$dmg" -nobrowse -mountpoint "$mnt"
  app="$(find "$mnt" -maxdepth 2 -name '*.app' -print | head -n 1 || true)"
  if [[ -z "$app" ]]; then
    hdiutil detach "$mnt" >/dev/null 2>&1 || true
    rmdir "$mnt" 2>/dev/null || true
    say "That disk image does not contain an app."
    exit 1
  fi
  install_macos_app "$app"
  hdiutil detach "$mnt" >/dev/null 2>&1 || true
  rmdir "$mnt" 2>/dev/null || true
}

install_appimage() {
  local src="$1" dest desktop
  dest="${HOME}/.local/bin/LetsWritePaper.AppImage"
  desktop="${HOME}/.local/share/applications/letswritepaper.desktop"
  mkdir -p "${HOME}/.local/bin" "${HOME}/.local/share/applications"
  cp "$src" "$dest"
  chmod +x "$dest"
  cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=LetsWritePaper
Comment=Write papers with co-authors and AI agents
Exec=${dest}
Terminal=false
Categories=Office;Education;
EOF
  installed_path="$dest"
  say "Installed ${dest}"
  say "It is also in your application menu."
}

install_deb() {
  local pkg="$1"
  say "Installing the system package. Your password lets the package manager write outside your home folder."
  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y "$pkg"
  else
    sudo dpkg -i "$pkg"
  fi
  installed_path="letswritepaper"
}

install_rpm() {
  local pkg="$1"
  say "Installing the system package. Your password lets the package manager write outside your home folder."
  if command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y "$pkg"
  elif command -v zypper >/dev/null 2>&1; then
    sudo zypper install -y "$pkg"
  else
    sudo rpm -U "$pkg"
  fi
  installed_path="letswritepaper"
}

install_payload() {
  local path="$1" name
  name="$(lower "$path")"
  case "$name" in
    *.app) install_macos_app "$path" ;;
    *.dmg) install_dmg "$path" ;;
    *.appimage) install_appimage "$path" ;;
    *.deb) install_deb "$path" ;;
    *.rpm) install_rpm "$path" ;;
    *)
      say "Unrecognized installer: $path"
      exit 1
      ;;
  esac
}

download_to() {
  local url="$1" dest
  dest="$work/$(basename "${url%%\?*}")"
  say "Downloading $(basename "$dest")"
  curl -fL --retry 3 --output "$dest" "$url"
  printf '%s\n' "$dest"
}

scan_dir() {
  local bundle="$1"
  found_items=()
  [[ -d "$bundle" ]] || return 0
  local line
  while IFS= read -r line; do
    [[ -n "$line" ]] && found_items+=("$line")
  done < <(find "$bundle" -maxdepth 3 \( -name '*.app' -o -name '*.dmg' -o -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' \) -print)
}

pick_from() {
  user_best=""
  pkg_best=""
  if [[ "$#" -eq 0 ]]; then
    return 0
  fi
  split_installers "$@"
  if [[ "${#user_items[@]}" -gt 0 ]]; then
    user_best="$(best_scored "${user_items[@]}")"
  fi
  if [[ "${#pkg_items[@]}" -gt 0 ]]; then
    pkg_best="$(best_scored "${pkg_items[@]}")"
  fi
}

choose_one() {
  chosen=""
  if [[ -n "$user_best" && -n "$pkg_best" ]]; then
    say "1) For this user only"
    say "   $user_best"
    say "2) System package (asks for your password)"
    say "   $pkg_best"
    case "$(choose "Choice" "1")" in
      2) chosen="$pkg_best" ;;
      *) chosen="$user_best" ;;
    esac
  elif [[ -n "$user_best" ]]; then
    chosen="$user_best"
  elif [[ -n "$pkg_best" ]]; then
    chosen="$pkg_best"
  fi
}

build_app() {
  command -v cargo >/dev/null 2>&1 || {
    say "Install Rust from https://rustup.rs and run this again."
    exit 1
  }
  command -v pnpm >/dev/null 2>&1 || {
    say "Install pnpm from https://pnpm.io and run this again."
    exit 1
  }
  say "Building the desktop app. The first build can take several minutes."
  (cd "$root/app" && pnpm install && pnpm tauri build)
}

# The directory of this script when it is a real file inside the LetsWritePaper
# source tree. Empty when the script is piped in, so a download never treats
# the current folder as the project.
source_root() {
  local src="${BASH_SOURCE[0]}"
  if [[ -f "$src" ]]; then
    local dir
    dir="$(cd "$(dirname "$src")" && pwd)"
    if [[ -f "$dir/app/src-tauri/tauri.conf.json" ]]; then
      printf '%s\n' "$dir"
    fi
  fi
}

resolve_repo() {
  if [[ -n "${LWP_GITHUB:-}" ]]; then
    printf '%s\n' "$LWP_GITHUB"
    return
  fi
  if [[ -n "${root}" && -d "${root}/.git" ]]; then
    local url repo
    url="$(git -C "$root" remote get-url origin 2>/dev/null || true)"
    repo="$(repo_from_url "$url")"
    if [[ -n "$repo" ]]; then
      printf '%s\n' "$repo"
      return
    fi
  fi
  printf '%s\n' "$DEFAULT_REPO"
}

main() {
  root="$(source_root)"
  case "$(uname -s)" in
    Darwin|Linux) ;;
    *)
      say "On Windows, download and run LetsWritePaper-x64-setup.exe:"
      say "https://github.com/${DEFAULT_REPO}/releases/latest/download/LetsWritePaper-x64-setup.exe"
      exit 1
      ;;
  esac

  say "LetsWritePaper"
  say ""
  say "This installs the desktop app for your user account."
  say "The first time it opens, a setup wizard asks for your name."
  say "That wizard can install tectonic for LaTeX and Claude Code."
  say "Typst preview is already included."
  say ""

  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT

  user_items=()
  pkg_items=()
  found_items=()
  user_best=""
  pkg_best=""
  chosen=""

  if [[ -n "$root" ]]; then
    scan_dir "$root/app/src-tauri/target/release/bundle"
  fi
  if [[ "${#found_items[@]}" -gt 0 ]]; then
    say "Using an installer built in this folder."
    pick_from "${found_items[@]}"
  else
    repo="$(resolve_repo)"
    remote_items=()
    say "Downloading the latest release from ${repo}…"
    while IFS= read -r line; do
      [[ -n "$line" ]] && remote_items+=("$line")
    done < <(release_urls "$repo")
    if [[ "${#remote_items[@]}" -gt 0 ]]; then
      pick_from "${remote_items[@]}"
    fi
  fi

  choose_one
  if [[ -z "$chosen" && -n "$root" && -f "$root/app/src-tauri/tauri.conf.json" ]]; then
    if is_yes "$(choose "No installer was found. Build one from this folder? This needs Rust, Node, and pnpm" "N")"; then
      build_app
      scan_dir "$root/app/src-tauri/target/release/bundle"
      if [[ "${#found_items[@]}" -gt 0 ]]; then
        pick_from "${found_items[@]}"
        choose_one
      fi
    fi
  fi

  if [[ -z "$chosen" ]]; then
    say "No installer was found for ${DEFAULT_REPO}."
    say "Publish a v* tag, or download a file from https://github.com/${DEFAULT_REPO}/releases/latest"
    exit 1
  fi

  say ""
  say "Installer: $chosen"
  if ! is_yes "$(choose "Install now?" "Y")"; then
    say "Nothing was installed."
    exit 0
  fi

  case "$chosen" in
    http://*|https://*) chosen="$(download_to "$chosen")" ;;
  esac
  install_payload "$chosen"

  say ""
  say "Open LetsWritePaper and finish the setup wizard."
  if [[ "$(uname -s)" == Darwin && -d "$installed_path" ]]; then
    say "If macOS asks, confirm the app in Privacy & Security the first time it opens."
    if is_yes "$(choose "Open LetsWritePaper now?" "Y")"; then
      open "$installed_path"
    fi
  elif [[ "$(uname -s)" == Linux && -x "$installed_path" ]]; then
    if is_yes "$(choose "Open LetsWritePaper now?" "Y")"; then
      "$installed_path" >/dev/null 2>&1 &
    fi
  fi
}

main "$@"
