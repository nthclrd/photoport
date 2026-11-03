// Thumbnail loading order: the iPhone serves ~25 thumbnails/s, so what is on
// screen goes first (closest to the middle first), what is near the screen
// next, and tiles scrolled away before their turn are never requested.

const MAX_IN_FLIGHT = 8;
const PREFETCH_MARGIN = "1200px 0px";

type Entry = { img: HTMLImageElement; url: string; near: boolean; loading: boolean; done?: () => void };

const waiting = new Map<HTMLImageElement, Entry>();
let inFlight = 0;

const observer = new IntersectionObserver(
  (records) => {
    for (const r of records) {
      const e = waiting.get(r.target as HTMLImageElement);
      if (e) e.near = r.isIntersecting;
    }
    pump();
  },
  { rootMargin: PREFETCH_MARGIN },
);

function distanceToMiddle(img: HTMLImageElement): number {
  const r = img.getBoundingClientRect();
  return Math.abs((r.top + r.bottom) / 2 - window.innerHeight / 2);
}

function pump() {
  while (inFlight < MAX_IN_FLIGHT) {
    let best: Entry | undefined;
    let bestDistance = Infinity;
    for (const e of waiting.values()) {
      if (!e.near) continue;
      const d = distanceToMiddle(e.img);
      if (d < bestDistance) {
        bestDistance = d;
        best = e;
      }
    }
    if (!best) return;
    start(best);
  }
}

function start(e: Entry) {
  waiting.delete(e.img);
  observer.unobserve(e.img);
  e.loading = true;
  inFlight++;
  const done = () => {
    if (!e.loading) return;
    e.loading = false;
    inFlight--;
    pump();
  };
  e.done = done;
  e.img.addEventListener("load", done, { once: true });
  e.img.addEventListener("error", done, { once: true });
  e.img.src = e.url;
}

/** Svelte action: `<img use:thumb={url}>` instead of `src`. */
export function thumb(img: HTMLImageElement, url: string) {
  const e: Entry = { img, url, near: false, loading: false };
  waiting.set(img, e);
  observer.observe(img);
  return {
    update(next: string) {
      if (next === e.url) return;
      e.url = next;
      if (!waiting.has(img)) img.src = next;
    },
    destroy() {
      if (waiting.delete(img)) observer.unobserve(img);
      // A tile removed mid-load must free its slot.
      e.done?.();
    },
  };
}
