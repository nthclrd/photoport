import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DeviceInfo = { udid: string; name: string; model: string; ios_version: string };

export type Help = {
  os: "windows" | "linux" | "macos" | "other";
  commands: string[];
  store_uri: string | null;
  web_url: string | null;
};

export type Status = (
  | { state: "usbmuxd_missing" }
  | { state: "no_device" }
  | { state: "not_paired"; device: DeviceInfo }
  | { state: "ready"; device: DeviceInfo }
) & { help: Help; importing: boolean; heic_converter: "sips" | "heif_convert" | null };

export type Item = {
  id: string;
  files: string[];
  kind: "photo" | "video" | "live";
  size: number;
  mtime: number | null;
  imported: boolean;
};

export type ScanSummary = {
  total_files: number;
  total_bytes: number;
  new_files: number;
  new_bytes: number;
  skipped: number;
  items: Item[];
};

export type Format = "original" | "jpg" | "both";
export type Layout = "month" | "day" | "year" | "flat";

export type ImportOptions = {
  dst: string;
  delete_after: boolean;
  format: Format;
  quality: number;
  layout: Layout;
  selection: string[] | null;
  lang: string;
};

export type Progress =
  | { kind: "Scanned"; total_files: number; new_files: number; new_bytes: number; skipped: number }
  | { kind: "Start"; total_files: number; total_bytes: number }
  | { kind: "FileStart"; path: string; size: number }
  | { kind: "FileProgress"; bytes_delta: number }
  | { kind: "FileDone"; path: string; size: number }
  | { kind: "FileError"; path: string; error: string };

export type Report = {
  ok: number;
  failed: number;
  deleted: number;
  bytes: number;
  secs: number;
  cancelled: boolean;
  errors: string[];
};

export const deviceStatus = () => invoke<Status>("device_status");
export const scanDevice = (dst: string) => invoke<ScanSummary>("scan_device", { dst });
export const startImport = (opts: ImportOptions) => invoke<void>("start_import", { opts });
export const cancelImport = () => invoke<void>("cancel_import");
export const openAppleDevices = () => invoke<void>("open_apple_devices");
export const openFolder = (path: string) => invoke<void>("open_folder", { path });
export const installUpdate = () => invoke<void>("install_update");

export const isWindows = navigator.userAgent.includes("Windows");
export const isMac = navigator.userAgent.includes("Mac");

// Custom protocols are served from http://<scheme>.localhost on Windows.
const base = (scheme: string) => (isWindows ? `http://${scheme}.localhost` : `${scheme}://localhost`);
const encodePath = (p: string) => p.split("/").map(encodeURIComponent).join("/");
type Mocks = { __photoportThumb?: (id: string) => string; __photoportMedia?: (path: string) => string };
const mocks = window as unknown as Mocks;

/** `item` gives size and date so a changed file gets a fresh cache entry. */
export function thumbUrl(item: Pick<Item, "id" | "size" | "mtime">): string {
  if (mocks.__photoportThumb) return mocks.__photoportThumb(item.id);
  return `${base("thumb")}${encodePath(item.id)}?v=${item.size}-${item.mtime ?? 0}`;
}

export function mediaUrl(path: string): string {
  if (mocks.__photoportMedia) return mocks.__photoportMedia(path);
  return base("media") + encodePath(path);
}

export function extOf(path: string): string {
  return (path.split(".").pop() ?? "").toLowerCase();
}

export type Suggestion = { label: string; kind: "place" | "scene"; count: number };
export const smartPrepare = () => invoke<{ suggestions: Suggestion[] }>("smart_prepare");
export const smartSearch = (query: string) => invoke<string[]>("smart_search", { query });

export function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => cb(e.payload));
}

export function humanBytes(n: number): string {
  const u = ["o", "Ko", "Mo", "Go", "To"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i + 1 < u.length) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${u[i]}`;
}

export function humanDuration(secs: number): string {
  if (!isFinite(secs) || secs < 0) return "…";
  const s = Math.round(secs);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ${String(s % 60).padStart(2, "0")}`;
  return `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, "0")}`;
}
