// Debrid analyze rewrites thumbnails.vtt after every chunk, so the seek bar
// picks up new sprites by polling until the file stops changing. 404 means not
// generated yet; 410 means the session is gone and nothing will ever change.
const VTT_POLL_MS = 15_000;
const VTT_STABLE_TICKS = 2;

export function pollThumbnailsVtt(url: string, onVtt: (vtt: string) => void): () => void {
  let cancelled = false;
  let lastVtt = '';
  let stableTicks = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const poll = async () => {
    if (cancelled) return;
    try {
      const r = await fetch(url);
      if (cancelled || r.status === 410) return;
      if (r.ok) {
        const vtt = await r.text();
        if (cancelled) return;
        if (vtt !== lastVtt) {
          onVtt(vtt);
          lastVtt = vtt;
          stableTicks = 0;
        } else if (++stableTicks >= VTT_STABLE_TICKS) {
          return;
        }
      }
    } catch {
      // Transient network error: try again next tick.
    }
    if (!cancelled) timer = setTimeout(poll, VTT_POLL_MS);
  };

  void poll();

  return () => {
    cancelled = true;
    if (timer !== null) clearTimeout(timer);
  };
}
