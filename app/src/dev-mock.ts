// Dev-only fake backend so the UI can be checked in a plain browser:
// npm run dev, then http://localhost:1420/?mock=ready (or no_device,
// not_paired, missing_windows, missing_linux). Never bundled in production.
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

const scenario = new URLSearchParams(location.search).get("mock") ?? "ready";
const device = { udid: "00000000-0000000000000000", name: "iPhone de Démo", model: "iPhone 16 Pro", ios_version: "18.6" };
const help = scenario === "missing_linux"
  ? { os: "linux", commands: ["sudo apt install usbmuxd        # Debian, Ubuntu, Mint", "sudo systemctl start usbmuxd"], store_uri: null, web_url: null }
  : { os: scenario === "missing_windows" ? "windows" : "macos", commands: [], store_uri: null, web_url: null };

type Item = { id: string; files: string[]; kind: "photo" | "video" | "live"; size: number; mtime: number; imported: boolean };

// ~300 items over five months, newest first; the oldest 40 already imported.
const items: Item[] = [];
const now = Math.floor(new Date("2025-06-20T12:00:00").getTime() / 1000);
for (let i = 0; i < 300; i++) {
  const n = String(1300 - i).padStart(4, "0");
  const kind = i % 11 === 0 ? "video" : i % 4 === 0 ? "live" : "photo";
  const ext = kind === "video" ? "MOV" : i % 13 === 0 ? "DNG" : i % 9 === 0 ? "PNG" : "HEIC";
  const id = `/DCIM/10${Math.floor(i / 100)}APPLE/IMG_${n}.${ext}`;
  const files = kind === "live" ? [id, id.replace(".HEIC", ".MOV")] : [id];
  const size = kind === "video" ? 40 * 1048576 : kind === "live" ? 5.5 * 1048576 : 2.6 * 1048576;
  items.push({ id, files, kind, size, mtime: now - i * 45000, imported: i >= 260 });
}

(window as unknown as { __photoportThumb: (id: string) => string }).__photoportThumb = (id) => {
  const n = Number(id.match(/IMG_(\d+)/)?.[1] ?? 0);
  const hue = (n * 47) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="360" height="480"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue},55%,62%)"/><stop offset="1" stop-color="hsl(${(hue + 60) % 360},50%,32%)"/></linearGradient></defs><rect width="360" height="480" fill="url(#g)"/></svg>`;
  return "data:image/svg+xml," + encodeURIComponent(svg);
};

(window as unknown as { __photoportMedia: (p: string) => string }).__photoportMedia = (p) =>
  (window as unknown as { __photoportThumb: (id: string) => string }).__photoportThumb(p);

const labels: Record<string, (i: number) => boolean> = {
  Guadeloupe: (i) => i % 5 === 0,
  Plage: (i) => i % 7 === 0,
  "New York": (i) => i % 23 === 0,
  Vélo: (i) => i % 17 === 0,
};
const norm = (s: string) => s.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();

function status() {
  const base = { help, importing: false, heic_converter: help.os === "windows" ? null : "sips" };
  switch (scenario) {
    case "no_device": return { state: "no_device", ...base };
    case "not_paired": return { state: "not_paired", device, ...base };
    case "missing_windows":
    case "missing_linux": return { state: "usbmuxd_missing", ...base };
    default: return { state: "ready", device, ...base };
  }
}

function summary() {
  const fresh = items.filter((i) => !i.imported);
  const files = (l: Item[]) => l.reduce((n, i) => n + i.files.length, 0);
  const bytes = (l: Item[]) => l.reduce((n, i) => n + i.size, 0);
  return {
    total_files: files(items), total_bytes: bytes(items),
    new_files: files(fresh), new_bytes: bytes(fresh), skipped: files(items) - files(fresh),
    items: items.map((i) => ({ ...i })),
  };
}

async function fakeImport(selection: string[]) {
  const picked = items.filter((i) => !i.imported && selection.includes(i.id));
  const files = picked.flatMap((i) => i.files.map((f) => ({ f, size: i.size / i.files.length })));
  const total = files.reduce((n, x) => n + x.size, 0);
  await emit("photoport://progress", { kind: "Start", total_files: files.length, total_bytes: total });
  for (const { f, size } of files) {
    await emit("photoport://progress", { kind: "FileStart", path: f, size });
    await emit("photoport://progress", { kind: "FileProgress", bytes_delta: size });
    await emit("photoport://progress", { kind: "FileDone", path: f, size });
    await new Promise((r) => setTimeout(r, 30));
  }
  picked.forEach((i) => (i.imported = true));
  await emit("photoport://done", { ok: files.length, failed: 0, deleted: 0, bytes: total, secs: files.length * 0.03, cancelled: false, errors: [] });
}

mockIPC(async (cmd, args) => {
  switch (cmd) {
    case "device_status": return status();
    case "scan_device": {
      await new Promise((r) => setTimeout(r, 1500));
      const total = items.length * 8;
      for (let done = 0; done <= total; done += 40) {
        await emit("photoport://scan", { done, total });
        await new Promise((r) => setTimeout(r, 40));
      }
      return summary();
    }
    case "start_import": {
      const opts = (args as { opts: { selection: string[] | null } }).opts;
      void fakeImport(opts.selection ?? items.map((i) => i.id));
      return null;
    }
    case "smart_prepare":
      await new Promise((r) => setTimeout(r, 700));
      return { suggestions: [
        { label: "Guadeloupe", kind: "place", count: 60 }, { label: "New York", kind: "place", count: 13 },
        { label: "Plage", kind: "scene", count: 43 }, { label: "Vélo", kind: "scene", count: 18 },
      ] };
    case "smart_search": {
      const words = norm((args as { query: string }).query).split(/\s+/).filter(Boolean);
      return items
        .filter((_, i) => words.every((w) => Object.entries(labels).some(([l, f]) => norm(l).split(" ").some((x) => x.startsWith(w)) && f(i))))
        .flatMap((it) => it.files);
    }
    case "plugin:dialog|open": return "/Users/demo/Images/iPhone";
    case "plugin:dialog|ask": return true;
    default: return null;
  }
}, { shouldMockEvents: true });

localStorage.setItem("photoport.dst", localStorage.getItem("photoport.dst") || "/Users/demo/Images/iPhone");
