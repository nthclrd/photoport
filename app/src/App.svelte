<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { open, ask } from "@tauri-apps/plugin-dialog";
  import { t, currentLang } from "./lib/i18n";
  import Grid from "./lib/Grid.svelte";
  import {
    deviceStatus, scanDevice, startImport, cancelImport, openAppleDevices, openFolder,
    installUpdate, on, humanBytes, humanDuration,
    type Status, type ScanSummary, type Progress, type Report, type Format, type Layout,
  } from "./lib/api";

  const saved = (k: string, d: string) => localStorage.getItem(`photoport.${k}`) ?? d;

  let status: Status | null = null;
  let dst = saved("dst", "");
  let format: Format = saved("format", "original") as Format;
  let quality = Number(saved("quality", "92"));
  let layout: Layout = saved("layout", "month") as Layout;
  let showImported = saved("showImported", "1") === "1";
  let deleteAfter = false;

  // Grid selection. Ids already seen keep the user's choice across rescans;
  // ids never seen before (new photos) start selected.
  let selected = new Set<string>();
  let knownIds = new Set<string>();

  let summary: ScanSummary | null = null;
  let scanning = false;
  let scanDone = 0, scanTotal = 0, scanStartedAt = 0;
  let scanError = "";

  let importing = false;
  let cancelling = false;
  let totalFiles = 0, doneFiles = 0, totalBytes = 0, doneBytes = 0, startedAt = 0;
  let currentFile = "";
  let report: Report | null = null;
  let fatal = "";
  let updateVersion = "";
  let copiedIdx = -1;
  let now = Date.now();

  let unlisten: Array<() => void> = [];
  let timer: number | undefined;

  $: localStorage.setItem("photoport.dst", dst);
  $: localStorage.setItem("photoport.format", format);
  $: localStorage.setItem("photoport.quality", String(quality));
  $: localStorage.setItem("photoport.layout", layout);
  $: localStorage.setItem("photoport.showImported", showImported ? "1" : "0");

  $: chosen = summary ? summary.items.filter((i) => !i.imported && selected.has(i.id)) : [];
  $: chosenBytes = chosen.reduce((n, i) => n + i.size, 0);
  $: chosenFiles = chosen.reduce((n, i) => n + i.files.length, 0);

  $: ready = status?.state === "ready";
  $: canConvert = !!status?.heic_converter;
  $: elapsed = startedAt ? (now - startedAt) / 1000 : 0;
  $: speed = elapsed > 0.5 ? doneBytes / elapsed : 0;
  $: eta = speed > 0 ? (totalBytes - doneBytes) / speed : Infinity;
  $: pct = totalBytes > 0 ? Math.min(100, (doneBytes / totalBytes) * 100) : 0;
  $: scanPct = scanTotal > 0 ? (scanDone / scanTotal) * 100 : 0;
  $: scanEta = scanStartedAt && scanDone > 20 && now > scanStartedAt
    ? ((scanTotal - scanDone) * (now - scanStartedAt)) / scanDone / 1000
    : Infinity;

  async function refreshStatus() {
    try {
      const prev = status?.state;
      status = await deviceStatus();
      if (status.state === "ready" && prev !== "ready" && dst && !importing) scan();
      if (status.state !== "ready" && !importing) summary = null;
    } catch (e) {
      console.error(e);
    }
  }

  async function pickFolder() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      if (picked !== dst) knownIds = new Set();
      dst = picked;
      if (ready) scan();
    }
  }

  async function scan() {
    if (!dst || scanning) return;
    scanning = true;
    scanError = "";
    scanDone = 0;
    scanTotal = 0;
    scanStartedAt = 0;
    try {
      const s = await scanDevice(dst);
      const next = new Set<string>();
      for (const it of s.items) {
        if (!it.imported && (selected.has(it.id) || !knownIds.has(it.id))) next.add(it.id);
      }
      selected = next;
      knownIds = new Set(s.items.map((i) => i.id));
      summary = s;
    } catch (e) {
      scanError = String(e);
    } finally {
      scanning = false;
    }
  }

  async function doImport() {
    if (!summary || chosen.length === 0) return;
    if (deleteAfter) {
      const ok = await ask(t("delete_after_confirm", { n: chosenFiles }), {
        title: t("delete_after_confirm_title"),
        kind: "warning",
      });
      if (!ok) return;
    }
    report = null;
    fatal = "";
    importing = true;
    cancelling = false;
    totalFiles = chosenFiles;
    totalBytes = chosenBytes;
    doneFiles = 0;
    doneBytes = 0;
    startedAt = 0;
    try {
      await startImport({
        dst,
        delete_after: deleteAfter,
        format: canConvert ? format : "original",
        quality,
        layout,
        selection: chosen.map((i) => i.id),
        lang: currentLang,
      });
    } catch (e) {
      importing = false;
      fatal = String(e);
    }
  }

  async function doCancel() {
    cancelling = true;
    await cancelImport();
  }

  function onProgress(p: Progress) {
    switch (p.kind) {
      case "Start":
        totalFiles = p.total_files;
        totalBytes = p.total_bytes;
        startedAt = Date.now();
        break;
      case "FileStart":
        currentFile = p.path.split("/").pop() ?? p.path;
        break;
      case "FileProgress":
        doneBytes += p.bytes_delta;
        break;
      case "FileDone":
      case "FileError":
        doneFiles += 1;
        break;
    }
  }

  async function copy(text: string, i: number) {
    await navigator.clipboard.writeText(text.split("#")[0].trim());
    copiedIdx = i;
    setTimeout(() => (copiedIdx = -1), 1500);
  }

  onMount(async () => {
    unlisten.push(await on<Progress>("photoport://progress", onProgress));
    unlisten.push(await on<{ done: number; total: number }>("photoport://scan", (p) => {
      if (p.total > 0 && !scanStartedAt) scanStartedAt = Date.now();
      scanDone = p.done;
      scanTotal = p.total;
    }));
    unlisten.push(await on<Report>("photoport://done", (r) => {
      report = r;
      importing = false;
      currentFile = "";
      deleteAfter = false;
      scan();
    }));
    unlisten.push(await on<string>("photoport://failed", (e) => {
      fatal = e;
      importing = false;
    }));
    unlisten.push(await on<string>("photoport://update", (v) => (updateVersion = v)));
    await refreshStatus();
    timer = window.setInterval(() => {
      if (!importing) refreshStatus();
    }, 1500);
    window.setInterval(() => (now = Date.now()), 500);
  });

  onDestroy(() => {
    unlisten.forEach((u) => u());
    if (timer) clearInterval(timer);
  });
</script>

<main>
  <header>
    <h1>{t("app_title")}</h1>
    <p class="muted">{t("tagline")}</p>
  </header>

  {#if updateVersion}
    <div class="card row between">
      <span>{t("update_available", { v: updateVersion })}</span>
      <button on:click={installUpdate}>{t("update_install")}</button>
    </div>
  {/if}

  {#if !status}
    <div class="card muted">…</div>
  {:else if status.state === "usbmuxd_missing"}
    <section class="card col warn">
      {#if status.help.os === "windows"}
        <h2>{t("driver_title_windows")}</h2>
        <p>{t("driver_body_windows")}</p>
        <button class="primary" on:click={openAppleDevices}>{t("driver_button_windows")}</button>
      {:else if status.help.os === "linux"}
        <h2>{t("driver_title_linux")}</h2>
        <p>{t("driver_body_linux")}</p>
        {#each status.help.commands as cmd, i}
          <div class="row cmd">
            <code>{cmd}</code>
            <button on:click={() => copy(cmd, i)}>{copiedIdx === i ? t("copied") : t("copy")}</button>
          </div>
        {/each}
      {:else}
        <h2>{t("driver_title_other")}</h2>
        <p>{t("driver_body_other")}</p>
      {/if}
    </section>
  {:else}
    <section class="card device">
      <div class="dot" class:on={ready} class:wait={status.state === "not_paired"}></div>
      <div class="col tight">
        {#if status.state === "no_device"}
          <strong>{t("state_no_device")}</strong>
          <span class="muted small">{t("state_no_device_hint")}</span>
        {:else if status.state === "not_paired"}
          <strong>{t("state_not_paired")}</strong>
          <span class="muted small">{t("state_not_paired_hint")}</span>
        {:else}
          <strong>{status.device.name}</strong>
          <span class="muted small">
            {status.device.model}{status.device.ios_version ? " · " + t("ios", { v: status.device.ios_version }) : ""}
          </span>
        {/if}
      </div>
    </section>
  {/if}

  <section class="card col">
    <label class="small muted" for="dst">{t("destination")}</label>
    <div class="row">
      <input id="dst" type="text" readonly value={dst} placeholder={t("destination_placeholder")} on:click={pickFolder} />
      <button on:click={pickFolder} disabled={importing}>{t("choose")}</button>
      {#if dst}<button on:click={() => openFolder(dst)}>{t("open_folder")}</button>{/if}
    </div>
    {#if ready && dst}
      <div class="row between small">
        <span class="muted">
          {#if scanning}{t("scanning")}
          {:else if scanError}<span class="err">{scanError}</span>
          {:else if summary && summary.new_files === 0}{t("nothing_new")}
          {:else if summary}{t("scan_summary", {
              total: summary.total_files, size: humanBytes(summary.total_bytes),
              new: summary.new_files, newsize: humanBytes(summary.new_bytes) })}
          {/if}
        </span>
        <button class="link" on:click={scan} disabled={scanning || importing}>{t("rescan")}</button>
      </div>
    {/if}
  </section>

  {#if scanning && ready}
    <section class="card col">
      <div class="row between">
        <strong>{t("scan_title")}</strong>
        <span class="small muted">
          {#if scanTotal}{t("scan_progress", { done: scanDone, total: scanTotal })}{#if isFinite(scanEta)}{" · " + t("scan_eta", { eta: humanDuration(scanEta) })}{/if}
          {:else}{t("scan_listing")}{/if}
        </span>
      </div>
      <div class="bar" class:indeterminate={!scanTotal}><div class="fill" style="width:{scanTotal ? scanPct : 30}%"></div></div>
      <span class="small muted">{summary ? t("scan_hint_again") : t("scan_hint_first")}</span>
    </section>
  {/if}

  {#if report}
    <section class="card col" class:warn={report.failed > 0}>
      <strong>{report.cancelled ? t("cancelled_title") : t("done_title")}</strong>
      <span>{t("done_body", { ok: report.ok, size: humanBytes(report.bytes), time: humanDuration(report.secs) })}</span>
      {#if report.failed > 0}<span class="small">{t("done_failed", { n: report.failed })}</span>{/if}
      {#if report.deleted > 0}<span class="small">{t("done_deleted", { n: report.deleted })}</span>{/if}
      <div><button on:click={() => openFolder(dst)}>{t("open_folder")}</button></div>
    </section>
  {/if}

  {#if fatal}
    <section class="card warn col"><strong>{t("error_title")}</strong><span class="small">{fatal}</span></section>
  {/if}
  <details class="card">
    <summary>{t("options")}</summary>
    <div class="col options">
      <fieldset class="col tight" disabled={importing}>
        <legend class="small muted">{t("layout")}</legend>
        {#each ["month", "day", "year", "flat"] as key}
          <label>
            <input type="radio" bind:group={layout} value={key} /> {t(`layout_${key}`)}
            <span class="small muted">· {t(`layout_${key}_ex`)}</span>
          </label>
        {/each}
      </fieldset>
      <fieldset class="col tight" disabled={!canConvert || importing}>
        <legend class="small muted">{t("format")}</legend>
        <label><input type="radio" bind:group={format} value="original" /> {t("format_original")}</label>
        <label><input type="radio" bind:group={format} value="jpg" /> {t("format_jpg")}</label>
        <label><input type="radio" bind:group={format} value="both" /> {t("format_both")}</label>
        <label class="row">
          <span class="small muted">{t("quality")}</span>
          <input type="range" min="50" max="100" bind:value={quality} disabled={format === "original"} />
          <span class="small">{quality}</span>
        </label>
      </fieldset>
      {#if !canConvert && status}
        <p class="small muted">
          {status.help.os === "windows" ? t("format_unavailable_windows") : t("format_unavailable_linux")}
        </p>
      {/if}
      <label class="col tight">
        <span class="row"><input type="checkbox" bind:checked={deleteAfter} disabled={importing} /> {t("delete_after")}</span>
        <span class="small muted indent">{t("delete_after_hint")}</span>
      </label>
    </div>
  </details>

  {#if ready && summary && summary.items.length > 0}
    <Grid items={summary.items} bind:selected bind:showImported disabled={importing || scanning} />
  {/if}

  <div class="actionbar">
  {#if !importing}
    <button class="primary big" on:click={doImport} disabled={!ready || !summary || chosen.length === 0 || scanning}>
      {chosen.length > 0 ? t("import_button", { n: chosen.length, size: humanBytes(chosenBytes) }) : t("import_button_none")}
    </button>
  {:else}
    <section class="card col">
      <div class="row between">
        <strong>{t("importing")}</strong>
        <span class="small muted">{t("progress_files", { done: doneFiles, total: totalFiles })}</span>
      </div>
      <progress value={pct} max="100"></progress>
      <div class="row between small">
        <span>{humanBytes(doneBytes)} / {humanBytes(totalBytes)}</span>
        <span class="muted">{speed > 0 ? t("progress_speed", { speed: humanBytes(speed), eta: humanDuration(eta) }) : ""}</span>
      </div>
      <div class="row between">
        <span class="small muted ellipsis">{currentFile}</span>
        <button class="danger" on:click={doCancel} disabled={cancelling}>{cancelling ? t("cancelling") : t("cancel")}</button>
      </div>
    </section>
  {/if}
  </div>
</main>
