// In-app updates: ask GitHub for a newer signed release on launch and offer to
// install it. A failed check (offline, rate-limited) stays silent.
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { h, showModal, toast } from "./ui";

export async function checkForUpdates() {
  if (import.meta.env.DEV) return;
  const update = await check().catch(() => null);
  if (!update) return;
  showModal(
    `LetsWritePaper ${update.version} is available`,
    h(
      "div",
      {},
      h("p", {}, `You have version ${update.currentVersion}. The update downloads in the background and the app restarts to finish. Your projects are not touched.`),
      update.body ? h("pre", { class: "update-notes" }, update.body) : null,
    ),
    [["Update and restart", () => void install(update)]],
  );
}

async function install(update: Update) {
  toast(`Downloading LetsWritePaper ${update.version}…`);
  try {
    await update.downloadAndInstall();
    await relaunch();
  } catch (e) {
    toast(`The update could not be installed: ${e}`, "error");
  }
}
