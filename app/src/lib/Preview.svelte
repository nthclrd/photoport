<script lang="ts">
  import { createEventDispatcher, onMount, onDestroy } from "svelte";
  import { t, currentLang } from "./i18n";
  import { thumbUrl, mediaUrl, extOf, isMac, humanBytes, type Item } from "./api";

  export let items: Item[] = [];
  export let index = 0;
  export let selected: Set<string>;
  export let disabled = false;

  const dispatch = createEventDispatcher<{ close: void; toggle: Item }>();
  const dateFmt = new Intl.DateTimeFormat(currentLang, { dateStyle: "long", timeStyle: "short" });

  let fullLoaded = false;
  let fullFailed = false;
  let videoFailed = false;
  let playingLive = false;
  let buffering = false;
  let width = 0;
  let height = 0;

  $: item = items[Math.min(index, items.length - 1)];
  $: if (item) reset(item.id);
  $: ext = item ? extOf(item.id) : "";
  $: livePath = item?.kind === "live" ? item.files.find((f) => ["mov", "mp4"].includes(extOf(f))) : undefined;
  $: tryOriginal = ["jpg", "jpeg", "png", "gif"].includes(ext) || (["heic", "heif"].includes(ext) && isMac);
  $: note =
    item?.kind === "video" && videoFailed ? t("preview_video_unsupported")
    : ext === "dng" ? t("preview_raw")
    : (["heic", "heif"].includes(ext) && !isMac) || fullFailed ? t("preview_thumb_only")
    : "";
  $: resolution = videoLabel(width, height);

  function reset(_id: string) {
    fullLoaded = false;
    fullFailed = false;
    videoFailed = false;
    playingLive = false;
    buffering = false;
    width = 0;
    height = 0;
  }

  function videoLabel(w: number, h: number) {
    const short = Math.min(w, h);
    if (!w || item?.kind === "photo") return "";
    if (short >= 2160) return "4K";
    if (short >= 1080) return "1080p";
    if (short >= 720) return "720p";
    return "";
  }

  function onFullLoad(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    fullLoaded = true;
    width = img.naturalWidth;
    height = img.naturalHeight;
  }

  function go(step: number) {
    const next = index + step;
    if (next >= 0 && next < items.length) index = next;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") dispatch("close");
    else if (e.key === "ArrowRight") go(1);
    else if (e.key === "ArrowLeft") go(-1);
    else if (e.key === " " && item && !item.imported && !disabled) {
      e.preventDefault();
      dispatch("toggle", item);
    } else return;
    e.preventDefault();
  }

  onMount(() => window.addEventListener("keydown", onKey));
  onDestroy(() => window.removeEventListener("keydown", onKey));
</script>

{#if item}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
  <div class="backdrop" on:click|self={() => dispatch("close")}>
    <button class="nav prev" on:click={() => go(-1)} disabled={index === 0} aria-label={t("preview_prev")}>‹</button>
    <div class="stage">
      {#key item.id}
        {#if item.kind === "video" && !videoFailed}
          <!-- svelte-ignore a11y-media-has-caption -->
          <video
            src={mediaUrl(item.id)}
            poster={thumbUrl(item)}
            controls
            autoplay
            playsinline
            on:loadedmetadata={(e) => ((width = e.currentTarget.videoWidth), (height = e.currentTarget.videoHeight))}
            on:error={() => ((videoFailed = true), (buffering = false))}
            on:loadstart={() => (buffering = true)}
            on:waiting={() => (buffering = true)}
            on:canplay={() => (buffering = false)}
            on:playing={() => (buffering = false)}
          ></video>
          {#if buffering}<div class="spinner" aria-label={t("preview_loading")}></div>{/if}
        {:else}
          <img class="media" class:hidden={fullLoaded} src={thumbUrl(item)} alt="" />
          {#if tryOriginal && !fullFailed}
            <img
              class="media"
              class:hidden={!fullLoaded}
              src={mediaUrl(item.id)}
              alt=""
              on:load={onFullLoad}
              on:error={() => (fullFailed = true)}
            />
          {/if}
          {#if playingLive && livePath}
            <!-- svelte-ignore a11y-media-has-caption -->
            <video class="media live" src={mediaUrl(livePath)} autoplay playsinline on:ended={() => (playingLive = false)} on:error={() => (playingLive = false)}></video>
          {/if}
        {/if}
      {/key}
    </div>
    <button class="nav next" on:click={() => go(1)} disabled={index >= items.length - 1} aria-label={t("preview_next")}>›</button>
    <button class="close" on:click={() => dispatch("close")} aria-label={t("preview_close")}>✕</button>

    <div class="info">
      <div class="col tight">
        <strong>{item.id.split("/").pop()}</strong>
        <span class="small">
          {ext.toUpperCase()}{resolution ? ` · ${resolution}` : ""}{width ? ` · ${width}×${height}` : ""} · {humanBytes(item.size)}{item.mtime ? ` · ${dateFmt.format(new Date(item.mtime * 1000))}` : ""}
        </span>
        {#if note}<span class="small note">{note}</span>{/if}
      </div>
      <div class="row">
        {#if livePath && !playingLive}
          <button on:click={() => (playingLive = true)}>▶ {t("preview_play_live")}</button>
        {/if}
        {#if item.imported}
          <span class="small">✓ {t("imported_badge")}</span>
        {:else}
          <button class:primary={selected.has(item.id)} disabled={disabled} on:click={() => dispatch("toggle", item)}>
            {selected.has(item.id) ? `✓ ${t("preview_selected")}` : t("preview_select")}
          </button>
        {/if}
        <span class="small muted">{index + 1} / {items.length}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    background: rgba(0, 0, 0, 0.92);
    display: grid;
    grid-template-columns: 64px 1fr 64px;
    grid-template-rows: 1fr auto;
    color: #eee;
  }
  .stage {
    position: relative;
    grid-column: 2;
    grid-row: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    padding: 20px 0;
  }
  .media, video {
    grid-area: 1 / 1;
    max-width: 100%;
    max-height: calc(100vh - 140px);
    object-fit: contain;
    border-radius: 6px;
  }
  .hidden { visibility: hidden; }
  .live { z-index: 1; }
  .nav {
    align-self: center;
    font-size: 34px;
    line-height: 1;
    background: none;
    border: none;
    color: #fff;
    padding: 12px;
  }
  .prev { grid-column: 1; grid-row: 1; }
  .next { grid-column: 3; grid-row: 1; }
  .close {
    position: absolute;
    top: 14px;
    right: 16px;
    background: rgba(255, 255, 255, 0.12);
    border: none;
    color: #fff;
  }
  .info {
    grid-column: 1 / -1;
    grid-row: 2;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 12px 24px 18px;
    background: rgba(0, 0, 0, 0.5);
  }
  .info button { background: rgba(255, 255, 255, 0.1); color: #fff; border-color: transparent; }
  .info button.primary { background: var(--accent); }
  .note { color: #f2c46b; }
  .spinner {
    grid-area: 1 / 1;
    z-index: 2;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    border: 4px solid rgba(255, 255, 255, 0.25);
    border-top-color: #fff;
    animation: spin 0.8s linear infinite;
    pointer-events: none;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
