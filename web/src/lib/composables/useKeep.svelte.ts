import { getKeepStatus, getRecentStreams, keepStream } from '$lib/api/client';
import type { KeepState } from '$lib/api/client';
import type { HeroAction } from '$lib/components/shared/HeroActions.svelte';

const KEEP_POLL_MS = 2_000;
const RUNNING: KeepState[] = ['queued', 'downloading', 'importing'];

type Phase = 'idle' | 'saving' | 'saved';

export async function findStreamSession(mediaId: string): Promise<string | null> {
  try {
    const { sessions } = await getRecentStreams();
    return sessions.find((s) => s.media_id === mediaId)?.id ?? null;
  } catch {
    return null;
  }
}

export function useKeep(opts: { onMessage: (msg: string) => void }) {
  let phase = $state<Phase>('idle');
  let progress = $state(0);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let generation = 0;

  function stop() {
    generation++;
    if (timer) clearTimeout(timer);
    timer = null;
  }

  function fail(message: string) {
    stop();
    phase = 'idle';
    progress = 0;
    opts.onMessage(message);
  }

  function apply(sessionId: string, state: KeepState, value?: number, error?: string) {
    if (state === 'done') {
      stop();
      phase = 'saved';
      return;
    }
    if (!RUNNING.includes(state)) return fail(error ?? "Couldn't save to your library");
    phase = 'saving';
    progress = value ?? progress;
    const gen = generation;
    timer = setTimeout(() => void poll(sessionId, gen), KEEP_POLL_MS);
  }

  async function poll(sessionId: string, gen: number) {
    try {
      const status = await getKeepStatus(sessionId);
      if (gen === generation) apply(sessionId, status.state, status.progress, status.error);
    } catch (e) {
      if (gen === generation)
        fail(e instanceof Error ? e.message : "Couldn't save to your library");
    }
  }

  async function keep(sessionId: string) {
    if (phase !== 'idle') return;
    stop();
    phase = 'saving';
    progress = 0;
    const gen = generation;
    try {
      const res = await keepStream(sessionId);
      if (gen === generation) apply(sessionId, res.state);
    } catch (e) {
      if (gen === generation)
        fail(e instanceof Error ? e.message : "Couldn't save to your library");
    }
  }

  async function resume(sessionId: string) {
    stop();
    phase = 'idle';
    progress = 0;
    const gen = generation;
    try {
      const status = await getKeepStatus(sessionId);
      if (gen === generation && (status.state === 'done' || RUNNING.includes(status.state))) {
        apply(sessionId, status.state, status.progress);
      }
    } catch {
      // Never kept: the action stays on offer.
    }
  }

  function label() {
    if (phase === 'saved') return 'Saved';
    if (phase === 'idle') return 'Keep in library';
    const pct = Math.round(progress * 100);
    return pct > 0 ? `Saving to library… ${pct}%` : 'Saving to library…';
  }

  return {
    get label() {
      return label();
    },
    get busy() {
      return phase === 'saving';
    },
    get saved() {
      return phase === 'saved';
    },
    keep,
    resume,
    dispose: stop
  };
}

export function useTitleKeep(onMessage: (msg: string) => void) {
  const keep = useKeep({ onMessage });
  let sessionId = $state<string | null>(null);
  let asked: string | null = null;

  async function refresh(mediaId: string) {
    asked = mediaId;
    const found = await findStreamSession(mediaId);
    if (asked !== mediaId) return;
    sessionId = found;
    if (found) await keep.resume(found);
  }

  return {
    get action(): HeroAction | null {
      const id = sessionId;
      if (!id) return null;
      return {
        label: keep.label,
        icon: keep.saved ? 'library_add_check' : 'library_add',
        title: 'Save the streamed file into your library',
        busy: keep.busy,
        disabled: keep.saved,
        onSelect: () => void keep.keep(id)
      };
    },
    refresh,
    dispose: keep.dispose
  };
}
