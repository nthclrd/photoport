<script lang="ts">
  import { onDestroy } from "svelte";
  import { t, currentLang } from "./i18n";
  import { thumbUrl, extOf, humanBytes, smartPrepare, smartSearch, type Item, type Suggestion } from "./api";
  import { thumb } from "./thumbs";
  import Preview from "./Preview.svelte";

  export let items: Item[] = [];
  export let selected: Set<string> = new Set();
  export let showImported = true;
  export let disabled = false;

  type TypeFilter = "all" | "photo" | "video" | "live" | "raw";
  const TYPES: TypeFilter[] = ["all", "photo", "video", "live", "raw"];
  let typeFilter: TypeFilter = "all";

  // Smart search (places and scenes recognised by iOS).
  let query = "";
  let smartState: "idle" | "loading" | "ready" | "error" = "idle";
  let smartError = "";
  let suggestions: Suggestion[] = [];
  let matched: Set<string> | null = null;
  let searchTimer: number | undefined;

  // Index in `visible` of the last clicked tile, for Shift+click ranges.
  let anchor = -1;
  let previewIndex = -1;

  const monthFmt = new Intl.DateTimeFormat(currentLang, { month: "long", year: "numeric" });

  const isRaw = (it: Item) => it.files.some((f) => extOf(f) === "dng");
  const typeOf = (it: Item): Exclude<TypeFilter, "all"> =>
    isRaw(it) ? "raw" : it.kind === "video" ? "video" : it.kind === "live" ? "live" : "photo";

  $: byPath = new Map(items.flatMap((it) => it.files.map((f) => [f.toLowerCase(), it.id] as const)));
  $: typeCounts = countTypes(items, showImported);
  $: visible = items.filter(
    (i) =>
      (showImported || !i.imported) &&
      (typeFilter === "all" || typeOf(i) === typeFilter) &&
      (!matched || matched.has(i.id)),
  );
  $: filtered = typeFilter !== "all" || matched !== null;
  $: position = new Map(visible.map((it, i) => [it.id, i]));
  $: sections = byMonth(visible);
  $: visibleOpen = visible.filter((i) => !i.imported);
  $: selectedOpen = items.filter((i) => !i.imported && selected.has(i.id));
  $: selectedBytes = selectedOpen.reduce((n, i) => n + i.size, 0);
  $: hiddenSelected = filtered ? [...selected].filter((id) => !position.has(id)).length : 0;

  function countTypes(list: Item[], withImported: boolean) {
    const c: Record<TypeFilter, number> = { all: 0, photo: 0, video: 0, live: 0, raw: 0 };
    for (const it of list) {
      if (!withImported && it.imported) continue;
      c.all++;
      c[typeOf(it)]++;
    }
    return c;
  }

  type Section = { key: string; label: string; items: Item[] };

  // Items arrive newest first, so each month is one contiguous run.
  function byMonth(list: Item[]): Section[] {
    const out: Section[] = [];
    for (const it of list) {
      const d = it.mtime != null ? new Date(it.mtime * 1000) : null;
      const key = d ? `${d.getFullYear()}-${d.getMonth()}` : "unknown";
      const last = out[out.length - 1];
      if (last && last.key === key) {
        last.items.push(it);
      } else {
        const label = d ? capitalize(monthFmt.format(d)) : t("unknown_date");
        out.push({ key, label, items: [it] });
      }
    }
    return out;
  }

  function capitalize(s: string) {
    return s.charAt(0).toUpperCase() + s.slice(1);
  }

  function name(it: Item) {
    return it.id.split("/").pop() ?? it.id;
  }

  function badge(it: Item): string {
    if (isRaw(it)) return "RAW";
    if (it.kind === "video") return "▶";
    if (it.kind === "live") return "LIVE";
    const e = extOf(it.id);
    return e === "png" || e === "gif" ? e.toUpperCase() : "";
  }

  // ---- selection ----

  function toggle(it: Item) {
    if (disabled || it.imported) return;
    selected.has(it.id) ? selected.delete(it.id) : selected.add(it.id);
    selected = selected;
  }

  function click(it: Item, e: MouseEvent) {
    if (justDragged || disabled || it.imported) return;
    const i = position.get(it.id) ?? -1;
    const on = !selected.has(it.id);
    if (e.shiftKey && anchor >= 0 && i >= 0) {
      const [a, b] = anchor < i ? [anchor, i] : [i, anchor];
      for (let k = a; k <= b; k++) {
        const x = visible[k];
        if (!x.imported) on ? selected.add(x.id) : selected.delete(x.id);
      }
    } else {
      on ? selected.add(it.id) : selected.delete(it.id);
    }
    anchor = i;
    selected = selected;
  }

  function setMany(list: Item[], on: boolean) {
    for (const it of list) if (!it.imported) on ? selected.add(it.id) : selected.delete(it.id);
    selected = selected;
  }

  function clearAll() {
    selected = new Set();
  }

  function monthFull(sec: Section, sel: Set<string>) {
    const open = sec.items.filter((i) => !i.imported);
    return open.length > 0 && open.every((i) => sel.has(i.id));
  }

  // ---- rubber-band selection: drag on the grid to select (or, starting
  // on a selected tile, to deselect) everything the rectangle touches ----

  let monthsEl: HTMLElement;
  let band: { x0: number; y0: number; cx: number; cy: number; on: boolean; base: Set<string>; moved: boolean } | null = null;
  let bandBox = { left: 0, top: 0, width: 0, height: 0 };
  let justDragged = false;
  let frame = 0;
  const AUTOSCROLL_EDGE = 70;
  const ACTIONBAR = 90;

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0 || disabled || e.shiftKey) return;
    const target = e.target as HTMLElement;
    if (target.closest(".month-head") || target.closest(".open")) return;
    const tile = target.closest<HTMLElement>(".tile");
    const startSelected = !!tile && selected.has(tile.dataset.id ?? "");
    band = {
      x0: e.clientX + window.scrollX,
      y0: e.clientY + window.scrollY,
      cx: e.clientX,
      cy: e.clientY,
      on: !startSelected,
      base: new Set(selected),
      moved: false,
    };
    e.preventDefault();
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp, { once: true });
  }

  function onPointerMove(e: PointerEvent) {
    if (!band) return;
    band.cx = e.clientX;
    band.cy = e.clientY;
    if (!band.moved && Math.hypot(e.clientX + window.scrollX - band.x0, e.clientY + window.scrollY - band.y0) < 6) return;
    if (!band.moved) {
      band.moved = true;
      frame = requestAnimationFrame(tick);
    }
  }

  function tick() {
    if (!band) return;
    const bottomEdge = window.innerHeight - ACTIONBAR;
    if (band.cy > bottomEdge - AUTOSCROLL_EDGE) window.scrollBy(0, Math.min(30, (band.cy - bottomEdge + AUTOSCROLL_EDGE) / 3));
    else if (band.cy < AUTOSCROLL_EDGE) window.scrollBy(0, -Math.min(30, (AUTOSCROLL_EDGE - band.cy) / 3));

    const x1 = band.cx + window.scrollX;
    const y1 = band.cy + window.scrollY;
    const left = Math.min(band.x0, x1) - window.scrollX;
    const top = Math.min(band.y0, y1) - window.scrollY;
    bandBox = { left, top, width: Math.abs(x1 - band.x0), height: Math.abs(y1 - band.y0) };
    const right = left + bandBox.width;
    const bottom = top + bandBox.height;

    const hits = new Set<string>();
    for (const month of monthsEl.querySelectorAll<HTMLElement>(".month")) {
      const m = month.getBoundingClientRect();
      if (m.bottom < top || m.top > bottom) continue;
      for (const tile of month.querySelectorAll<HTMLElement>(".tile:not(.done)")) {
        const r = tile.getBoundingClientRect();
        if (r.right >= left && r.left <= right && r.bottom >= top && r.top <= bottom) hits.add(tile.dataset.id ?? "");
      }
    }
    const next = new Set(band.base);
    for (const id of hits) band.on ? next.add(id) : next.delete(id);
    selected = next;
    frame = requestAnimationFrame(tick);
  }

  function onPointerUp() {
    window.removeEventListener("pointermove", onPointerMove);
    cancelAnimationFrame(frame);
    if (band?.moved) {
      // The click that ends a drag must not toggle the tile under the pointer.
      justDragged = true;
      setTimeout(() => (justDragged = false));
    }
    band = null;
  }

  onDestroy(() => {
    window.removeEventListener("pointermove", onPointerMove);
    cancelAnimationFrame(frame);
    clearTimeout(searchTimer);
  });

  // ---- smart search ----

  async function prepareSmart() {
    if (smartState === "loading" || smartState === "ready") return;
    smartState = "loading";
    try {
      suggestions = (await smartPrepare()).suggestions;
      smartState = "ready";
      if (query) runSearch();
    } catch (e) {
      smartError = String(e);
      smartState = "error";
    }
  }

  function onQuery() {
    clearTimeout(searchTimer);
    if (!query.trim()) {
      matched = null;
      return;
    }
    searchTimer = window.setTimeout(runSearch, 200);
  }

  async function runSearch() {
    if (smartState !== "ready" || !query.trim()) return;
    const q = query;
    const paths = await smartSearch(q);
    if (q !== query) return;
    const ids = new Set<string>();
    for (const p of paths) {
      const id = byPath.get(p.toLowerCase());
      if (id) ids.add(id);
    }
    matched = ids;
  }

  function pick(s: Suggestion) {
    query = s.label;
    runSearch();
  }

  function clearSearch() {
    query = "";
    matched = null;
  }

  function openPreview(it: Item) {
    const i = position.get(it.id);
    if (i !== undefined) previewIndex = i;
  }

  function hideBrokenThumb(e: Event) {
    (e.currentTarget as HTMLImageElement).style.visibility = "hidden";
  }
</script>

<section class="card col grid-card">
  <div class="row between wrap">
    <div class="col tight">
      <strong>{t("grid_title")}</strong>
      <span class="small muted">
        {t("selected_count", { n: selectedOpen.length, size: humanBytes(selectedBytes) })}
        {#if hiddenSelected > 0}· {t("hidden_selected", { n: hiddenSelected })}{/if}
      </span>
    </div>
    <div class="row wrap">
      <label class="small row nowrap"><input type="checkbox" bind:checked={showImported} /> {t("show_imported")}</label>
      <button on:click={() => setMany(visibleOpen, true)} disabled={disabled || visibleOpen.length === 0}>
        {filtered ? t("select_shown", { n: visibleOpen.length }) : t("select_all")}
      </button>
      <button on:click={clearAll} disabled={disabled || selected.size === 0}>{t("select_none")}</button>
    </div>
  </div>

  <div class="row wrap">
    <div class="search">
      <span class="lens" aria-hidden="true">⌕</span>
      <input
        type="search"
        placeholder={t("search_placeholder")}
        bind:value={query}
        on:focus={prepareSmart}
        on:input={onQuery}
      />
      {#if query}<button class="link small" on:click={clearSearch}>{t("search_clear")}</button>{/if}
    </div>
    <div class="chips">
      {#each TYPES as ty}
        <button class="chip" class:on={typeFilter === ty} on:click={() => (typeFilter = ty)} disabled={typeCounts[ty] === 0 && ty !== "all"}>
          {t(`type_${ty}`)} <span class="muted">{typeCounts[ty]}</span>
        </button>
      {/each}
    </div>
  </div>

  {#if smartState === "loading"}
    <span class="small muted">{t("search_loading")}</span>
  {:else if smartState === "error"}
    <span class="small err">{t("search_unavailable")} ({smartError})</span>
  {:else if smartState === "ready" && !query}
    <div class="chips">
      {#each suggestions as s}
        <button class="chip small" on:click={() => pick(s)}>{s.kind === "place" ? "📍" : "✦"} {s.label}</button>
      {/each}
    </div>
  {:else if matched}
    <span class="small muted">{t("search_results", { n: visible.length, q: query })}</span>
  {/if}

  <span class="small muted">{t("grid_hint")}</span>

  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="months" bind:this={monthsEl} on:pointerdown={onPointerDown}>
    {#each sections as sec (sec.key)}
      <div class="month">
        <div class="row between month-head">
          <h3>{sec.label}</h3>
          {#if sec.items.some((i) => !i.imported)}
            <button class="link small" disabled={disabled} on:click={() => setMany(sec.items, !monthFull(sec, selected))}>
              {monthFull(sec, selected) ? t("unselect_month") : t("select_month")}
            </button>
          {/if}
        </div>
        <div class="tiles">
          {#each sec.items as it (it.id)}
            <button
              type="button"
              class="tile"
              class:sel={selected.has(it.id)}
              class:done={it.imported}
              data-id={it.id}
              title={`${name(it)} · ${humanBytes(it.size)}`}
              aria-pressed={selected.has(it.id)}
              on:click={(e) => click(it, e)}
              on:dblclick={() => openPreview(it)}
            >
              <span class="name">{name(it)}</span>
              <img use:thumb={thumbUrl(it)} alt="" decoding="async" draggable="false" on:error={hideBrokenThumb} />
              {#if badge(it)}<span class="badge kind">{badge(it)}</span>{/if}
              {#if it.imported}
                <span class="badge done-badge">✓ {t("imported_badge")}</span>
              {:else}
                <span class="check" aria-hidden="true"></span>
              {/if}
              <span
                class="open"
                role="button"
                tabindex="-1"
                title={t("preview_open")}
                on:click|stopPropagation={() => openPreview(it)}
                on:keydown={() => {}}
              >⤢</span>
            </button>
          {/each}
        </div>
      </div>
    {/each}
    {#if visible.length === 0}
      <p class="muted small empty">{t("no_match")}</p>
    {/if}
  </div>
</section>

{#if band?.moved}
  <div class="band" style="left:{bandBox.left}px;top:{bandBox.top}px;width:{bandBox.width}px;height:{bandBox.height}px"></div>
{/if}

{#if previewIndex >= 0}
  <Preview
    items={visible}
    bind:index={previewIndex}
    {selected}
    {disabled}
    on:close={() => (previewIndex = -1)}
    on:toggle={(e) => toggle(e.detail)}
  />
{/if}

<style>
  .wrap { flex-wrap: wrap; }
  .nowrap { white-space: nowrap; gap: 0; }
  .grid-card { gap: 10px; }
  .months { user-select: none; -webkit-user-select: none; }
  .month { content-visibility: auto; contain-intrinsic-size: auto 480px; }
  .month-head { margin: 10px 0 8px; }
  h3 { margin: 0; font-size: 14px; font-weight: 600; }
  .empty { padding: 20px 0; text-align: center; }

  .search {
    flex: 1 1 260px;
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0 10px;
  }
  .search input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    color: var(--fg);
    font: inherit;
    padding: 7px 0;
    outline: none;
  }
  .lens { color: var(--muted); font-size: 16px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip { padding: 4px 10px; border-radius: 999px; }
  .chip.on { background: var(--accent); color: var(--accent-fg); border-color: transparent; }
  .chip.on .muted { color: inherit; opacity: 0.8; }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(108px, 1fr));
    gap: 4px;
  }
  .tile {
    position: relative;
    aspect-ratio: 1;
    padding: 0;
    border: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--border);
    cursor: pointer;
  }
  .tile.done { cursor: default; }
  .tile img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    transition: transform 0.12s ease-out;
  }
  .tile .name {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 10.5px;
    color: var(--muted);
    padding: 4px;
    word-break: break-all;
  }
  .tile.sel { outline: 3px solid var(--accent); outline-offset: -3px; }
  .tile.sel img { transform: scale(0.92); border-radius: 4px; }
  .tile.done img { filter: grayscale(0.4) brightness(0.45); }
  .badge {
    position: absolute;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.03em;
    color: #fff;
    background: rgba(0, 0, 0, 0.55);
    border-radius: 4px;
    padding: 1px 5px;
  }
  .kind { left: 6px; bottom: 6px; }
  .done-badge { left: 6px; top: 6px; background: color-mix(in srgb, var(--ok) 80%, black); }
  .check {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.9);
    background: rgba(0, 0, 0, 0.2);
    box-shadow: 0 0 2px rgba(0, 0, 0, 0.5);
  }
  .tile.sel .check { background: var(--accent); border-color: #fff; }
  .tile.sel .check::after {
    content: "";
    position: absolute;
    left: 5px;
    top: 1px;
    width: 5px;
    height: 10px;
    border: solid #fff;
    border-width: 0 2px 2px 0;
    transform: rotate(45deg);
  }
  .open {
    position: absolute;
    right: 6px;
    bottom: 6px;
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    font-size: 14px;
    opacity: 0;
    transition: opacity 0.1s;
  }
  .tile:hover .open { opacity: 1; }
  .band {
    position: fixed;
    z-index: 10;
    pointer-events: none;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    border-radius: 2px;
  }
</style>
