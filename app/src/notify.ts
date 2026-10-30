import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";

/** A macOS notification, only while the window is in the background. */
export async function notify(title: string, body: string): Promise<void> {
  // A plain browser (vite dev without Tauri) has no notifications to send.
  if (!("__TAURI_INTERNALS__" in window) || document.hasFocus()) return;
  try {
    const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
  } catch {
    // A refused or missing notification changes nothing about the run.
  }
}
