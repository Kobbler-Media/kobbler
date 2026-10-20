import type { PlaybackSessionResponse } from '$lib/types';
import type { ClientCapabilities } from '$lib/utils/media';
import { detectCapabilities } from '$lib/utils/media';
import { createPlaybackSession, createIptvSession, destroyPlaybackSession } from '$lib/api/client';
import { showMessage } from '$lib/context/app.svelte';
import { invalidate } from '$app/navigation';
import { bumpWatchStateVersion } from '$lib/stores/watchState.svelte';

export function usePlayback(opts: {
  mediaType: 'movie' | 'episode' | 'live' | 'recording';
  channelId?: string;
  refreshWatchState: () => Promise<void>;
}) {
  let playbackSession = $state<PlaybackSessionResponse | null>(null);
  let startingPlayback = $state<string | null>(null);
  let playingFileId = $state<string | null>(null);
  let playbackCapabilities = $state<ClientCapabilities | null>(null);

  // Pending state: player is open but waiting for a stream to resolve
  let pending = $state(false);

  // Pre-warm state: a session created in advance for instant playback
  let preWarmFileId: string | null = null;
  let preWarmSession: PlaybackSessionResponse | null = null;
  let preWarmCapabilities: ClientCapabilities | null = null;
  let preWarmPromise: Promise<void> | null = null;

  async function play(fileId: string, sourceId?: string, resumePosition?: number) {
    startingPlayback = fileId;
    try {
      // If a pre-warm is in flight for this file, await it instead of
      // creating a duplicate session.
      if (preWarmFileId === fileId && preWarmPromise) {
        console.log('[Playback] Joining in-flight pre-warm for', fileId);
        await preWarmPromise;
      }

      // If we have a pre-warmed session for this file, promote it —
      // but only if the start position matches what we need now.
      if (preWarmFileId === fileId && preWarmSession && preWarmCapabilities) {
        const preWarmStart = preWarmSession.start_time ?? 0;
        const wantedStart = resumePosition && resumePosition > 5 ? resumePosition : 0;
        if (Math.abs(preWarmStart - wantedStart) < 10) {
          console.log('[Playback] Using pre-warmed session', preWarmSession.session_id);
          playingFileId = fileId;
          playbackCapabilities = preWarmCapabilities;
          playbackSession = preWarmSession;
          preWarmFileId = null;
          preWarmSession = null;
          preWarmCapabilities = null;
          preWarmPromise = null;
          return;
        }
        console.log(
          '[Playback] Position mismatch — preWarm:',
          preWarmStart,
          'wanted:',
          wantedStart
        );
        // Position mismatch — tear down and create fresh with correct start_time
        teardownPreWarmQuietly();
      } else if (preWarmFileId !== fileId) {
        console.log('[Playback] Pre-warm file mismatch — preWarm:', preWarmFileId, 'play:', fileId);
      } else {
        console.log(
          '[Playback] No pre-warm available — preWarmSession:',
          !!preWarmSession,
          'preWarmCaps:',
          !!preWarmCapabilities
        );
      }

      // Tear down any stale pre-warm (user played a different file)
      teardownPreWarmQuietly();

      // Open player immediately in pending/loading state while we create the session.
      // This makes the player chrome appear instantly instead of waiting for the API.
      pending = true;

      const capabilities = detectCapabilities();
      // Pass resume position as start_time so the transcode starts
      // directly at the right position (no separate seek needed)
      const session =
        opts.mediaType === 'live'
          ? await createIptvSession({
              channel_id: opts.channelId ?? fileId,
              capabilities
            })
          : await createPlaybackSession({
              media_type: opts.mediaType,
              file_id: fileId,
              capabilities,
              ...(resumePosition != null && resumePosition > 5
                ? { start_time: resumePosition }
                : {})
            });
      pending = false;
      playingFileId = fileId;
      playbackCapabilities = capabilities;
      playbackSession = session;
    } catch (e) {
      pending = false;
      showMessage(e instanceof Error ? e.message : 'Failed to start playback');
    } finally {
      startingPlayback = null;
    }
  }

  async function close() {
    pending = false;
    playbackSession = null;
    playingFileId = null;
    playbackCapabilities = null;
    if (opts.mediaType !== 'live') {
      try {
        await opts.refreshWatchState();
      } catch {
        /* ignore */
      }
      invalidate('kob:watch-state').catch(() => {});
      bumpWatchStateVersion();
    }
  }

  /** Pre-warm a playback session so ffmpeg/transcode setup happens early. */
  function preWarm(fileId: string, resumePosition?: number) {
    // Don't pre-warm live streams or if already playing/pre-warming this file
    if (opts.mediaType === 'live') return;
    if (playingFileId === fileId || preWarmFileId === fileId) return;

    // Tear down any existing pre-warm for a different file
    teardownPreWarmQuietly();

    // Set fileId immediately to prevent duplicate calls while awaiting
    preWarmFileId = fileId;

    const doPreWarm = async () => {
      try {
        const capabilities = detectCapabilities();
        const session = await createPlaybackSession({
          media_type: opts.mediaType as 'movie' | 'episode' | 'recording',
          file_id: fileId,
          capabilities,
          ...(resumePosition != null && resumePosition > 5 ? { start_time: resumePosition } : {})
        });
        preWarmSession = session;
        preWarmCapabilities = capabilities;
      } catch {
        // Pre-warm is best-effort — don't show errors to the user
        preWarmFileId = null;
      } finally {
        preWarmPromise = null;
      }
    };

    preWarmPromise = doPreWarm();
  }

  function teardownPreWarmQuietly() {
    preWarmPromise = null;
    if (preWarmSession) {
      const sessionId = preWarmSession.session_id;
      preWarmFileId = null;
      preWarmSession = null;
      preWarmCapabilities = null;
      destroyPlaybackSession(sessionId).catch(() => {});
    }
  }

  /** Destroy the pre-warmed session. Call on unmount / navigation. */
  function teardownPreWarm() {
    teardownPreWarmQuietly();
  }

  function openPending() {
    teardownPreWarmQuietly();
    playbackSession = null;
    playingFileId = null;
    pending = true;
  }

  /** Start playback from a pre-built session (e.g. debrid stream resolve). */
  function playSession(session: PlaybackSessionResponse) {
    teardownPreWarmQuietly();
    pending = false;
    playbackSession = session;
    playingFileId = `debrid-${session.session_id}`;
    playbackCapabilities = detectCapabilities();
  }

  return {
    get session() {
      return playbackSession;
    },
    get startingFileId() {
      return startingPlayback;
    },
    get playingFileId() {
      return playingFileId;
    },
    get capabilities() {
      return playbackCapabilities;
    },
    get isActive() {
      return (
        pending ||
        (playbackSession !== null && playingFileId !== null && playbackCapabilities !== null)
      );
    },
    get isPending() {
      return pending;
    },
    get isLive() {
      return opts.mediaType === 'live';
    },
    play,
    playSession,
    openPending,
    close,
    preWarm,
    teardownPreWarm
  };
}
