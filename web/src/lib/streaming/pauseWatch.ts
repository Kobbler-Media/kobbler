import type { StreamPause } from '#lib/api/client.js';

export const PAUSE_FIRST_CHECK_MS = 1_500;
export const PAUSE_POLL_MS = 3_000;

const LEADS: Record<string, string> = {
  adds_paused: 'Your debrid service is pausing new requests on your account.',
  rate_limited: 'Your debrid service is limiting requests right now.'
};

export function pauseNote(pause: StreamPause): string {
  const lead = LEADS[pause.reason] ?? 'Your debrid service is making requests wait.';
  return `${lead} Trying again in about ${pause.seconds}s…`;
}

// A resolve can sit for half a minute while the debrid service holds the
// account back, and only the server knows that is why.
export function watchPause<T>(
  pending: Promise<T>,
  check: () => Promise<StreamPause | null>,
  onNote: (note: string | null) => void,
  signal: AbortSignal
): Promise<T> {
  let settled = false;
  let noted = false;
  let timer = setTimeout(poll, PAUSE_FIRST_CHECK_MS);

  function note(pause: StreamPause | null) {
    if (!pause && !noted) return;
    noted = pause !== null;
    onNote(pause && pauseNote(pause));
  }

  async function poll() {
    if (signal.aborted) return;
    const pause = await check();
    if (settled || signal.aborted) return;
    note(pause);
    timer = setTimeout(poll, PAUSE_POLL_MS);
  }

  return pending.finally(() => {
    settled = true;
    clearTimeout(timer);
    if (!signal.aborted) note(null);
  });
}
