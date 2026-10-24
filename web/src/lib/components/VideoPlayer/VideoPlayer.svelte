<script lang="ts">
  import { onMount, onDestroy, untrack, type Snippet } from 'svelte';
  import { uid } from '@koblibs/ui';
  import { registerOverlay, unregisterOverlay } from '$lib/stores/ui.svelte';
  import { enterPip, isPipActive, cleanupPip } from '$lib/stores/videopip.svelte';
  import {
    destroyPlaybackSession,
    createPlaybackSession,
    createIptvSession,
    seekPlayback,
    reportPlaybackDecodeError,
    switchAudioTrack,
    heartbeatSession,
    updateMoviePosition,
    updateEpisodePosition,
    updateMovieWatchState,
    updateEpisodeWatchState,
    getNextEpisode,
    getEpisodeFiles,
    getSeason,
    getPlaybackHandoff,
    getPlaybackSessionStatus,
    reportStreamOutcome
  } from '$lib/api/client';
  import { superviseStart } from '$lib/streaming/startLiveness';
  import { reconnectStep } from '$lib/streaming/reconnect';
  import { startingAudio } from '$lib/streaming/audioPreference';
  import { liveSyncRate, liveTargetLatency } from '$lib/streaming/liveLatency';
  import { useKeep } from '$lib/composables/useKeep.svelte';
  import type { CachingState } from '$lib/streaming/cacheWait';
  import StartShell from './StartShell.svelte';
  import { createNextEpisodeWarmer } from '$lib/streaming/episodeStream';
  import type { StreamPlayerFailure } from '$lib/composables/useStreamPlayback.svelte';
  import {
    on as onLibraryEvent,
    off as offLibraryEvent,
    onReconnect,
    offReconnect
  } from '$lib/services/eventstream.svelte';
  import { playbackClientId } from '$lib/utils/playback-client';
  import PlayerControls from './PlayerControls.svelte';
  import { SEGMENT_LABELS } from './segments';
  import { showMessage } from '$lib/context/app.svelte';
  import UpNextOverlay from './UpNextOverlay.svelte';
  import HandoffPrompt from './HandoffPrompt.svelte';
  import SubtitleSearchModal from '$lib/components/shared/SubtitleSearchModal.svelte';
  import type {
    PlaybackMethod,
    PlaybackSessionResponse,
    PlaybackSubtitleTrack,
    SkipSegment,
    NextEpisodeInfo,
    TvEpisode,
    WatchProgress
  } from '$lib/types';
  import type { ClientCapabilities } from '$lib/utils/media';
  import { getCastState, castLoadMedia, endCastSession } from '$lib/stores/cast.svelte';
  import { onStateChange, onMessage as onCastMessage, getDeviceName } from '$lib/cast/sender';
  import { castCapabilities } from '$lib/cast/capabilities';
  import { createPlaybackToken, revokePlaybackToken } from '$lib/api/client';
  import type { InitSessionPayload } from '$lib/cast/messages';
  import type Hls from 'hls.js';
  import { parseVtt } from '$lib/utils/vttSubtitles';
  import { replaceState } from '$app/navigation';

  interface MediaInfo {
    title: string;
    subtitle?: string;
    seasonId?: string;
    artworkUrl?: string;
    backdropUrl?: string;
  }

  interface Props {
    sessionResponse: PlaybackSessionResponse | null;
    creationParams?: {
      media_type: 'movie' | 'episode' | 'live' | 'recording';
      file_id: string;
      capabilities: ClientCapabilities;
    };
    mediaId?: string;
    mediaInfo?: MediaInfo;
    initialPosition?: number;
    resumePosition?: number;
    pendingStatus?: string;
    pendingCaching?: CachingState | null;
    onClose: () => void;
    onSwitchStream?: () => void;
    onPickStream?: () => void;
    /** Returns true when the host takes over (e.g. tries another release); false keeps the in-player error. */
    onStreamFailed?: (failure: StreamPlayerFailure) => boolean;
    /**
     * Fired when the player switches to a different episode in-place (Up Next,
     * episode list) so the host page can refresh its own metadata — title,
     * watch state, file list — which the in-place switch would otherwise leave
     * pointing at the episode the page was originally loaded for.
     */
    onEpisodeChange?: (episodeId: string) => void;
    /** Segment types the manual skip button offers. Omit to offer every visible type. */
    skipButtonTypes?: string[];
    /** Segment types skipped automatically on entry, with a toast so it stays recoverable. */
    autoSkipTypes?: string[];
    /**
     * Persist a boundary edit. Without it the editor writes to the library
     * intro/credits endpoints, which only understand movie and episode files.
     */
    onSaveSkipSegment?: (
      seg: SkipSegment,
      startSeconds: number,
      endSeconds: number
    ) => Promise<void>;
    onDeleteSkipSegment?: (seg: SkipSegment) => Promise<void>;
    /** Rendered above the controls bar while the controls are visible. */
    bottomPanel?: Snippet;
  }

  let {
    sessionResponse,
    creationParams: initialCreationParams,
    mediaId: initialMediaId,
    mediaInfo,
    initialPosition,
    resumePosition,
    pendingStatus,
    pendingCaching = null,
    onClose,
    onSwitchStream,
    onPickStream,
    onStreamFailed,
    onEpisodeChange,
    skipButtonTypes,
    autoSkipTypes,
    onSaveSkipSegment,
    onDeleteSkipSegment,
    bottomPanel
  }: Props = $props();

  // Default creation params for pending state (no session yet)
  const defaultCreationParams = {
    media_type: 'movie' as const,
    file_id: '',
    capabilities: {} as ClientCapabilities
  };

  // Mutable copies — updated when switching to next episode in-place
  let activeMediaId = $state(untrack(() => initialMediaId));
  let activeCreationParams = $state(
    untrack(() => (initialCreationParams ? { ...initialCreationParams } : defaultCreationParams))
  );
  let activeMediaInfo = $state<MediaInfo | undefined>(untrack(() => mediaInfo));

  /** Is this a debrid stream session? Debrid sessions can't be recreated via the local playback API. */
  let isDebridSession = $derived(activeCreationParams.file_id.startsWith('debrid-'));

  /** Destroy a session — debrid sessions are now real playback sessions. */
  async function maybeDestroySession(sessionId: string) {
    await destroyPlaybackSession(sessionId);
    // Reset so destroySessionOnce can handle the next session on unmount
    sessionDestroyed = false;
  }

  /** Create a new playback session, routing to the correct API for live vs VOD. */
  function recreateSession(extra?: {
    audio_track_index?: number;
    subtitle_track_index?: number;
  }): Promise<PlaybackSessionResponse> {
    if (activeCreationParams.media_type === 'live') {
      return createIptvSession({
        channel_id: activeCreationParams.file_id,
        capabilities: activeCreationParams.capabilities
      });
    }
    if (isDebridSession) {
      // Debrid streams are direct play from the debrid provider — no local session to recreate.
      // Return the existing session (stream URL is still valid).
      return Promise.resolve(session);
    }
    return createPlaybackSession({
      media_type: activeCreationParams.media_type,
      file_id: activeCreationParams.file_id,
      capabilities: activeCreationParams.capabilities,
      ...extra
    });
  }

  // Mutable session state — supports recreation for audio track switching
  // When pending (sessionResponse is null), session starts null and gets set via $effect
  let session = $state(untrack(() => sessionResponse ?? placeholderSession()));
  let sessionReady = $state(untrack(() => sessionResponse !== null));
  let isLive = $derived(
    session.duration_seconds === 0 && activeCreationParams.media_type === 'live'
  );

  function placeholderSession(): PlaybackSessionResponse {
    return {
      session_id: '',
      method: 'direct_play',
      playlist_url: null,
      direct_url: null,
      duration_seconds: 0,
      subtitles: [],
      audio_tracks: []
    };
  }

  // When sessionResponse transitions from null to a value, initialize the player.
  // Track by session_id so object-reference changes don't cause re-init.
  let initializedSessionId: string | null = null;
  $effect(() => {
    const sr = sessionResponse;
    if (!sr && untrack(() => sessionReady)) {
      untrack(returnToPending);
    } else if (sr && sr.session_id !== initializedSessionId) {
      session = sr;
      sessionReady = true;
      if (initialCreationParams) {
        activeCreationParams = { ...initialCreationParams };
      }
      // A discover stream mints its library entity on resolve, so the id only
      // exists once the session does.
      if (sr.media_id) activeMediaId = sr.media_id;
      duration = sr.duration_seconds;
      activeAudioIndex = startingAudio(
        sr.method,
        sr.audio_tracks,
        sr.preferred_audio_stream_index
      ).active;
      const resume = untrack(() => resumePosition);
      if (resume !== undefined) pendingResume = sr.start_time ? 0 : resume > 5 ? resume : 0;
      // Defer player initialization to next tick so DOM is ready after {#if sessionReady}
      queueMicrotask(() => {
        if (videoEl && HlsClass) {
          videoEl.playbackRate = playbackSpeed;
          initPlayer();
          startHeartbeat();
          watchDebridStart();
          resetHideTimer();
          if (sr.lufs != null && videoEl) {
            try {
              audioContext = new AudioContext();
              const source = audioContext.createMediaElementSource(videoEl);
              audioGainNode = audioContext.createGain();
              audioLimiterNode = createNormalizationLimiter(audioContext);
              source.connect(audioGainNode);
              audioGainNode.connect(audioLimiterNode);
              audioLimiterNode.connect(audioContext.destination);
              applyVideoNormalization();
            } catch (e) {
              console.warn('Web Audio setup failed:', e);
            }
          }
          if (activeCreationParams.media_type === 'episode' && activeMediaId) {
            getNextEpisode(activeMediaId).then((info) => {
              nextEpisode = info;
            });
            if (activeMediaInfo?.seasonId) {
              getSeason(activeMediaInfo.seasonId).then((season) => {
                seasonEpisodes = season.episodes;
              });
            }
          }
          startPositionSaving();
        }
      });
    }
    initializedSessionId = sr?.session_id ?? null;
  });

  let videoEl: HTMLVideoElement | undefined = $state();
  let containerEl: HTMLDivElement | undefined = $state();
  let hlsInstance: Hls | null = null;
  const LIVE_SYNC_DURATION_S = 20;
  /**
   * Refined `method` value seen on the SSE `probe_complete` event (debrid
   * sessions only). Set by the SSE handler; consumed by a $effect that
   * applies the change once the session is ready. See the probe_complete
   * comment in the SSE handler for why this is reactive rather than
   * applied inline.
   */
  let pendingMethodRefinement: PlaybackMethod | null = $state(null);
  // Actual PTS of the first segment from hls.js, used to calibrate seekOffset
  // so displayTime matches real content time (fixes intro marker drift).
  let initPtsSeconds: number | null = null;
  // Absolute time the user wants to land at after the next FRAG_BUFFERED.
  // Set by handleSeek; null on initial load (we still calibrate seekOffset
  // but don't jump within the segment — the user starts where FFmpeg started).
  let pendingCalibrationTarget: number | null = null;
  let pendingCalibrationServerOffset: number = 0;
  let jassubInstance: { destroy: () => void } | null = null;
  // Store HlsClass reference for reuse after seek (avoid re-importing)
  let HlsClass: typeof Hls | null = null;

  let playing = $state(false);
  let currentTime = $state(0);
  let duration = $state(untrack(() => sessionResponse?.duration_seconds ?? 0));
  let buffered = $state(0);
  let volume = $state(1);
  let muted = $state(false);
  let isFullscreen = $state(false);
  let loading = $state(true);
  let controlsVisible = $state(true);
  let activeSubtitleId = $state<string | null>(null);
  let activeVttTextTrack: TextTrack | null = null;
  let vttParsedCues: Array<{ start: number; end: number; text: string }> = [];
  let vttAppliedOffset: number | null = null;
  let error = $state<string | null>(null);
  let streamStarted = $state(false);
  let startProgress = $state<string | null>(null);
  let startWatch: AbortController | null = null;
  let startHeld = $state(false);
  let reloadedStallSession: string | null = null;
  let loadingMessage = $state<string | null>(null);
  let loadingTimer: ReturnType<typeof setTimeout> | null = null;
  let showSubtitleSearch = $state(false);
  let playbackSpeed = $state(
    typeof localStorage !== 'undefined'
      ? parseFloat(localStorage.getItem('video:playbackSpeed') || '1')
      : 1
  );
  let isPip = $state(false);
  let pipSupported = $state(
    typeof document !== 'undefined' &&
      'pictureInPictureEnabled' in document &&
      document.pictureInPictureEnabled
  );

  // Single source of PiP state. Toggling `isPip` is all we do here:
  // entering PiP keeps the component mounted (overlay hides via CSS), so
  // playback continues to live in this component's video element / HLS /
  // session. Ownership only transfers to the global videopip store if the
  // component is unmounted while PiP is active (see onDestroy +
  // transferToPipStore).
  $effect(() => {
    if (!videoEl) return;
    videoEl.setAttribute('autopictureinpicture', '');
    const onEnter = () => {
      isPip = true;
    };
    const onLeave = () => {
      isPip = false;
    };
    videoEl.addEventListener('enterpictureinpicture', onEnter);
    videoEl.addEventListener('leavepictureinpicture', onLeave);
    return () => {
      videoEl?.removeEventListener('enterpictureinpicture', onEnter);
      videoEl?.removeEventListener('leavepictureinpicture', onLeave);
    };
  });

  // ── Cast integration ───────────────────────────────────────
  const castState = getCastState();
  let castingMode = $state(false);
  const startArt = $derived(activeMediaInfo?.backdropUrl ?? activeMediaInfo?.artworkUrl);
  const showStartShell = $derived(
    !streamStarted &&
      !startHeld &&
      !error &&
      !castingMode &&
      activeCreationParams.media_type !== 'live'
  );
  let castPlaybackToken: string | null = null;
  let castUnsubscribe: (() => void) | null = null;
  /** Last known position from Cast receiver, used for resume on disconnect. */
  let lastCastPosition = 0;

  async function transferToCast() {
    if (!sessionReady || !session.session_id) return;

    // Pause local playback
    pauseElement();

    try {
      // Get a playback token for the receiver
      const { token } = await createPlaybackToken(24);
      castPlaybackToken = token;

      // Build the base URL for the receiver to call our API
      const baseUrl = window.location.origin;

      // Fully-qualify all relative URLs for the receiver
      const qualify = (url: string | null | undefined) =>
        url ? (url.startsWith('http') ? url : `${baseUrl}${url}`) : null;

      const payload: InitSessionPayload = {
        baseUrl,
        token,
        sessionId: session.session_id,
        mediaId: activeMediaId ?? null,
        mediaType: activeCreationParams.media_type,
        playlistUrl: qualify(session.playlist_url),
        directUrl: qualify(session.direct_url),
        method: session.playlist_url ? 'hls' : 'direct_play',
        duration: session.duration_seconds,
        subtitles: session.subtitles.map((s) => ({
          ...s,
          url: qualify(s.url)
        })),
        audioTracks: session.audio_tracks,
        thumbnailsUrl: qualify(session.thumbnails_url),
        chapters: session.chapters ?? [],
        skipSegments: session.skip_segments ?? [],
        capabilities: castCapabilities(getDeviceName() ?? ''),
        metadata: {
          title: activeMediaInfo?.title ?? 'Kobbler',
          subtitle: activeMediaInfo?.subtitle,
          posterUrl: activeMediaInfo?.artworkUrl
            ? (qualify(activeMediaInfo.artworkUrl) ?? undefined)
            : undefined
        }
      };

      castLoadMedia(payload);
      castingMode = true;
    } catch (err) {
      console.error('Failed to transfer to Cast:', err);
    }
  }

  async function transferFromCast() {
    castingMode = false;

    // Best position: live Cast state > last broadcast > local display time
    const resumeTime =
      castState.mediaState?.currentTime ?? (lastCastPosition > 0 ? lastCastPosition : displayTime);

    // Revoke the playback token
    if (castPlaybackToken) {
      revokePlaybackToken(castPlaybackToken).catch(() => {});
      castPlaybackToken = null;
    }

    // Resume local playback at the Cast position
    if (videoEl && resumeTime > 0) {
      handleSeek(resumeTime);
    }
    if (videoEl) {
      userPaused = false;
      videoEl.play().catch(() => {});
    }
  }

  // Web Audio volume normalization
  let audioContext: AudioContext | null = null;
  let audioGainNode: GainNode | null = null;
  // Brick-wall limiter sitting between gain and destination so amplified
  // peaks can't push past 0 dBFS into the DAC. Without it, a quietly-mastered
  // source (e.g. -25 LUFS TV) gets +11 dB of gain, which clips any sample
  // already near -1 dBFS — amplifying intersample peaks and source artifacts
  // into audible crackle. See LUFS normalization in `applyVideoNormalization`.
  let audioLimiterNode: DynamicsCompressorNode | null = null;
  let normalizeVolume = $state(
    typeof localStorage !== 'undefined'
      ? localStorage.getItem('video:normalizeVolume') !== 'false'
      : true
  );
  // Skip segment state
  let activeSkipSegment = $state<SkipSegment | null>(null);
  let skipDismissed = $state(false);
  let postCreditsScene = $derived(
    session.skip_segments?.find((s) => s.segment_type === 'post_credits_scene') ?? null
  );
  let editingSkipSegment = $state(false);
  let editSegment = $state<SkipSegment | null>(null);
  let editStart = $state(0);
  let editEnd = $state(0);

  function openSkipEditor(seg: SkipSegment) {
    editSegment = seg;
    editStart = seg.start_seconds;
    editEnd = seg.end_seconds;
    editingSkipSegment = true;
  }

  export function openSegmentEditor(seg: SkipSegment) {
    openSkipEditor(seg);
  }

  // Up Next state
  let nextEpisode = $state<NextEpisodeInfo | null>(null);
  let showUpNext = $state(false);
  let upNextDismissed = $state(false);

  // Cross-device handoff (see api/src/watch_handoff.rs). Paused here, carried on
  // elsewhere, come back: offer to follow the other device instead of letting
  // this player write its stale position back over the newer one. While an
  // offer is open this player saves nothing.
  const HANDOFF_STAY_SECONDS = 15;
  // A save this close to where we already are is the same place, not news.
  const HANDOFF_MIN_DELTA_SECONDS = 30;
  let handoff = $state<WatchProgress | null>(null);
  let handoffScopeId: string | null = null;
  // Server-clock ms of the newest save already accounted for. Only newer ones are
  // offered — so opening an older episode on purpose is not second-guessed.
  let handoffSince = 0;
  let handoffStayCountdown = $state<number | null>(null);
  let handoffStayTimer: ReturnType<typeof setInterval> | null = null;
  let videoEnded = $state(false);

  // Aspect ratio mode — 'auto' | 'fit' | 'fill'
  // auto: cover when content AR matches screen AR (within 15%), contain otherwise
  // fit: always object-fit: contain (letterbox)
  // fill: always object-fit: cover (crop to fill, draws behind notches)
  type AspectMode = 'auto' | 'fit' | 'fill';
  const ASPECT_STORAGE_KEY = 'kobbler-video-aspect-mode';
  let aspectMode = $state<AspectMode>(
    untrack(() => {
      if (typeof window === 'undefined') return 'auto';
      const v = localStorage.getItem(ASPECT_STORAGE_KEY);
      return v === 'fit' || v === 'fill' || v === 'auto' ? v : 'auto';
    })
  );
  let videoIntrinsicWidth = $state(0);
  let videoIntrinsicHeight = $state(0);
  let viewportWidth = $state(typeof window !== 'undefined' ? window.innerWidth : 0);
  let viewportHeight = $state(typeof window !== 'undefined' ? window.innerHeight : 0);
  const ASPECT_TOLERANCE = 0.15;
  let effectiveObjectFit = $derived.by(() => {
    if (aspectMode === 'fit') return 'contain';
    if (aspectMode === 'fill') return 'cover';
    if (videoIntrinsicWidth <= 0 || videoIntrinsicHeight <= 0) return 'contain';
    if (viewportWidth <= 0 || viewportHeight <= 0) return 'contain';
    const contentAR = videoIntrinsicWidth / videoIntrinsicHeight;
    const screenAR = viewportWidth / viewportHeight;
    const diff = Math.abs(contentAR - screenAR) / screenAR;
    return diff <= ASPECT_TOLERANCE ? 'cover' : 'contain';
  });

  function cycleAspectMode() {
    const order: AspectMode[] = ['auto', 'fit', 'fill'];
    const next = order[(order.indexOf(aspectMode) + 1) % order.length];
    aspectMode = next;
    if (typeof window !== 'undefined') {
      localStorage.setItem(ASPECT_STORAGE_KEY, next);
    }
  }

  function handleLoadedMetadata() {
    if (videoEl) {
      videoIntrinsicWidth = videoEl.videoWidth;
      videoIntrinsicHeight = videoEl.videoHeight;
    }
  }

  $effect(() => {
    if (typeof window === 'undefined') return;
    const onResize = () => {
      viewportWidth = window.innerWidth;
      viewportHeight = window.innerHeight;
    };
    window.addEventListener('resize', onResize);
    window.addEventListener('orientationchange', onResize);
    return () => {
      window.removeEventListener('resize', onResize);
      window.removeEventListener('orientationchange', onResize);
    };
  });

  // Season episodes for episode list panel
  let seasonEpisodes = $state<TvEpisode[]>([]);

  const nextWarmer = createNextEpisodeWarmer();
  const keep = useKeep({ onMessage: showMessage });

  // Per title, not per session: a source switch must not forget a keep in progress.
  let keepTitle: string | null = null;
  $effect(() => {
    const sessionId = session.session_id;
    const titleId = activeMediaId ?? sessionId;
    if (!isDebridSession || !sessionId || titleId === keepTitle) return;
    keepTitle = titleId;
    untrack(() => void keep.resume(sessionId));
  });
  const releaseNextWarm = () => nextWarmer.release(true);

  // Prevent overlapping server-side seeks — each one kills/restarts FFmpeg
  let serverSeeking = false;
  let pendingSeekTime: number | null = null;

  // Seek offset tracking — when FFmpeg restarts from a seek position,
  // videoEl.currentTime starts from 0 but the UI needs to show absolute time.
  // NOTE: with -c:v copy, fMP4 segments retain absolute PTS. Some hls.js
  // versions normalize currentTime to 0 (offset needed), others report
  // absolute PTS (offset must be 0). We detect which after each seek.
  let seekOffset = $state(untrack(() => session.start_time ?? 0));
  let displayTime = $derived(seekOffset + currentTime);
  let displayBuffered = $derived(seekOffset + buffered);
  /** Effective playback time — uses Cast receiver position when casting. */
  let effectiveTime = $derived(
    castingMode && castState.mediaState ? castState.mediaState.currentTime : displayTime
  );
  let effectiveDuration = $derived(
    castingMode && castState.mediaState ? castState.mediaState.duration : duration
  );

  // Audio track state
  let activeAudioIndex = $state<number | null>(
    untrack(() =>
      sessionResponse
        ? startingAudio(
            sessionResponse.method,
            sessionResponse.audio_tracks,
            sessionResponse.preferred_audio_stream_index
          ).active
        : null
    )
  );
  let audioSwitchedSession: string | null = null;

  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  const overlayId = `video-player-${uid()}`;

  // Position saving state
  // Seeded with where this player opened: the server already holds it, and a tab left
  // paused while another device moved on must not write it back over the newer save.
  let lastSavedPosition = untrack(() => Math.floor(initialPosition ?? 0));
  let positionSaveInterval: ReturnType<typeof setInterval> | null = null;
  let markedWatched = false;
  // If the server started the transcode at start_time, no client-side seek needed.
  // Set seekOffset so the UI shows the correct absolute time.
  let pendingResume = untrack(() => {
    if (session.start_time != null && session.start_time > 0) return 0;
    return initialPosition && initialPosition > 5 ? initialPosition : 0;
  });

  // Heartbeat + session recovery state
  let heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  let recovering = false;
  let unmounted = false;
  // Backoff schedule (ms before each create attempt). A backend restart wipes
  // the in-memory session, so recovery means minting a *fresh* session — but the
  // backend may still be coming up, so we retry with growing backoff rather than
  // giving up after a fixed count. The schedule length is the attempt budget.
  const RECOVERY_BACKOFFS_MS = [0, 3_000, 6_000, 12_000, 20_000];
  // recoverSession succeeds at *minting* a session even when playback is broken
  // (e.g. a seek into a never-produced segment). The fresh session resets the
  // per-instance hls retry counter, so a broken-playback loop would recover
  // forever. Cap recoveries within a rolling window and surface an error past it.
  let recoveryTimestamps: number[] = [];
  let reusedSessionId: string | null = null;
  const RECOVERY_WINDOW_MS = 90_000;
  const MAX_RECOVERIES_IN_WINDOW = 4;

  // The user's pause intent. Every path that rebuilds the media pipeline
  // (server-side seek, track switch, session recovery) ends in autoplay(), so
  // without this a paused player gets yanked back into playback by a seek or
  // an API restart. Only an explicit play — the play/pause control, a media
  // key, transferring back from Cast, an episode switch — clears it.
  let userPaused = false;
  // Pause events we caused ourselves (teardown, cast handover, end of file)
  // must not register as intent. Count them out rather than reading a flag at
  // the event: the `pause` event is queued, so it lands after any guard we
  // could reset synchronously. Reset on play, so a torn-down element that
  // never dispatched its pause can't swallow the next real one.
  let programmaticPauses = 0;

  function pauseElement() {
    if (!videoEl || videoEl.paused) return;
    programmaticPauses++;
    videoEl.pause();
  }

  function startHeartbeat() {
    stopHeartbeat();
    heartbeatInterval = setInterval(async () => {
      if (recovering) return;
      const sessionId = session.session_id;
      const alive = await heartbeatSession(sessionId);
      if (alive || recovering) return;
      if (isDebridSession) {
        // A debrid session cannot be minted again from here; its way back is the
        // release walk, and a heartbeat that merely failed to arrive is no news.
        if ((await getPlaybackSessionStatus(sessionId)).kind === 'missing') {
          stopHeartbeat();
          failPlayback('The stream ended on the server', 'session_lost');
        }
        return;
      }
      console.warn('[Player] Heartbeat: session lost, recovering...');
      recoverSession();
    }, 120_000); // 2 minutes
  }

  function stopHeartbeat() {
    if (heartbeatInterval) {
      clearInterval(heartbeatInterval);
      heartbeatInterval = null;
    }
  }

  async function recoverSession() {
    if (recovering) return;

    const nowMs = Date.now();
    recoveryTimestamps = recoveryTimestamps.filter((t) => nowMs - t < RECOVERY_WINDOW_MS);
    if (recoveryTimestamps.length >= MAX_RECOVERIES_IN_WINDOW) {
      console.warn('[Player] recovery storm — giving up to avoid an endless loop');
      failPlayback('Stream keeps failing — try a different source', 'recovery_storm');
      return;
    }
    recoveryTimestamps.push(nowMs);

    recovering = true;
    loading = true;
    error = null;

    const savedPosition = isLive ? 0 : displayTime;
    // Capture before cleanup() — it pauses the element and clears `playing`.
    // Live streams always resume playing; there's nothing to hold a position at.
    if (isLive) userPaused = false;
    else if (!playing) userPaused = true;
    const savedAudioIndex = activeAudioIndex;
    const savedSubStreamIndex = activeSubtitleId
      ? (session.subtitles.find((s) => s.track_id === activeSubtitleId)?.stream_index ?? undefined)
      : undefined;

    cleanup();
    const previousId = session.session_id;
    // Reopening the same session twice running means it is the session that is broken.
    let reusable = reusedSessionId !== previousId;

    let lastError: unknown = null;
    for (let attempt = 0; attempt < RECOVERY_BACKOFFS_MS.length; attempt++) {
      if (unmounted) {
        recovering = false;
        return;
      }
      const delay = RECOVERY_BACKOFFS_MS[attempt];
      if (delay > 0) await new Promise((resolve) => setTimeout(resolve, delay));
      if (unmounted) {
        recovering = false;
        return;
      }
      if (reusable) {
        const step = reconnectStep(await getPlaybackSessionStatus(previousId));
        if (step === 'wait') {
          lastError = new Error('The server could not be reached');
          continue;
        }
        if (step === 'reuse') {
          reusedSessionId = previousId;
          initPlayer();
          startHeartbeat();
          if (!isLive && savedPosition > 5) await handleSeek(savedPosition);
          recovering = false;
          return;
        }
        reusable = false;
        try {
          await maybeDestroySession(previousId);
        } catch {
          // Expected if the session already expired
        }
      }
      try {
        const newSession = await recreateSession({
          audio_track_index: savedAudioIndex ?? undefined,
          subtitle_track_index: savedSubStreamIndex
        });
        session = newSession;
        seekOffset = 0;

        // Map old subtitle to new session's tracks via stream_index
        if (savedSubStreamIndex != null) {
          const matchingSub = newSession.subtitles.find(
            (s) => s.stream_index === savedSubStreamIndex
          );
          activeSubtitleId = matchingSub?.track_id ?? null;
        }

        initPlayer();
        startHeartbeat();

        if (!isLive && savedPosition > 5) {
          await handleSeek(savedPosition);
        }
        recovering = false;
        return;
      } catch (e) {
        lastError = e;
        console.warn(
          `[Player] recovery attempt ${attempt + 1}/${RECOVERY_BACKOFFS_MS.length} failed`,
          e
        );
      }
    }

    recovering = false;
    failPlayback(
      lastError instanceof Error ? lastError.message : 'Stream failed — try a different source',
      'recovery_failed'
    );
  }

  function startPositionSaving() {
    if (positionSaveInterval) clearInterval(positionSaveInterval);
    positionSaveInterval = null;
    if (isLive) return;
    positionSaveInterval = setInterval(() => {
      savePosition();
      checkAutoMarkWatched();
    }, 30_000);
  }

  async function savePosition() {
    if (isLive || castingMode) return; // Receiver saves position directly when casting
    if (handoff) return; // Undecided: this position may be the stale one
    const pos = Math.floor(effectiveTime);
    if (pos <= 0 || Math.abs(pos - lastSavedPosition) <= 1 || !activeMediaId) return;
    lastSavedPosition = pos;
    try {
      if (activeCreationParams.media_type === 'movie') {
        await updateMoviePosition(activeMediaId, pos);
      } else if (activeCreationParams.media_type === 'episode') {
        await updateEpisodePosition(activeMediaId, pos);
      }
    } catch (e) {
      console.warn('[Player] Failed to save position:', e);
    }
  }

  async function checkAutoMarkWatched() {
    if (isLive || castingMode || markedWatched || effectiveDuration <= 0 || !activeMediaId) return;
    if (handoff) return;
    if (effectiveTime / effectiveDuration >= 0.9) {
      markedWatched = true;
      try {
        const now = new Date().toISOString();
        if (activeCreationParams.media_type === 'movie') {
          await updateMovieWatchState(activeMediaId, { watched: true, watched_at: now });
        } else if (activeCreationParams.media_type === 'episode') {
          await updateEpisodeWatchState(activeMediaId, { watched: true, watched_at: now });
        }
      } catch (e) {
        console.warn('[Player] Failed to auto-mark watched:', e);
      }
    }
  }

  // ── Media Session API (lock screen / now playing) ───────────
  function updateMediaSession() {
    if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) return;
    const info = activeMediaInfo;
    if (!info) return;

    const artwork: { src: string; sizes?: string; type?: string }[] = [];
    if (info.artworkUrl) {
      artwork.push({ src: info.artworkUrl, sizes: '512x512', type: 'image/jpeg' });
    }

    navigator.mediaSession.metadata = new MediaMetadata({
      title: info.subtitle ? `${info.title} - ${info.subtitle}` : info.title,
      artist: info.subtitle ? info.title : undefined,
      artwork
    });
    navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
  }

  function setupMediaSessionHandlers() {
    if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) return;
    navigator.mediaSession.setActionHandler('play', () => togglePlayPause());
    navigator.mediaSession.setActionHandler('pause', () => togglePlayPause());
    navigator.mediaSession.setActionHandler('seekbackward', () => {
      if (videoEl) handleSeek(Math.max(0, displayTime - 10));
    });
    navigator.mediaSession.setActionHandler('seekforward', () => {
      if (videoEl) handleSeek(displayTime + 30);
    });
    navigator.mediaSession.setActionHandler('seekto', (details) => {
      if (details.seekTime != null) handleSeek(details.seekTime);
    });
  }

  function updateMediaSessionPosition() {
    if (
      typeof navigator === 'undefined' ||
      !('mediaSession' in navigator) ||
      !duration ||
      !isFinite(duration)
    )
      return;
    navigator.mediaSession.setPositionState({
      duration,
      playbackRate: playbackSpeed,
      position: Math.min(displayTime, duration)
    });
  }

  // hls.js lazy-loaded module reference (HlsClass !== null means loaded)

  // Portal action: move the player overlay to document.body so it escapes
  // any containing block (backdrop-filter, transform) from parent components
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        // When PiP is active, the global store owns the overlay div — don't remove it
        if (!enteredPip) {
          node.remove();
        }
      }
    };
  }

  // Initialize default subtitle
  const defaultSub = untrack(() => sessionResponse?.subtitles.find((s) => s.is_default));
  if (defaultSub) {
    activeSubtitleId = defaultSub.track_id;
  }

  // Rebuild VTT text track when seekOffset changes (seek, PTS calibration)
  // so cue timestamps stay aligned with hls.js's normalized currentTime.
  $effect(() => {
    // Read seekOffset to establish reactivity
    const _offset = seekOffset;
    if (vttParsedCues.length > 0 && vttAppliedOffset !== _offset) {
      rebuildVttTextTrack();
    }
  });

  // Show "Preparing stream..." after 1s of loading for transcoded content
  $effect(() => {
    if (loading && session.method !== 'direct_play') {
      loadingTimer = setTimeout(() => {
        loadingMessage = 'Preparing stream\u2026';
      }, 1000);
    } else {
      if (loadingTimer) clearTimeout(loadingTimer);
      loadingTimer = null;
      loadingMessage = null;
    }
  });

  // Track active skip segment based on current playback position
  $effect(() => {
    if (skipDismissed || !session.skip_segments?.length) {
      activeSkipSegment = null;
      return;
    }
    const t = displayTime;
    const hit =
      session.skip_segments.find(
        (s) => s.segment_type !== 'post_credits_scene' && t >= s.start_seconds && t < s.end_seconds
      ) ?? null;
    activeSkipSegment =
      hit && skipButtonTypes && !skipButtonTypes.includes(hit.segment_type) ? null : hit;
  });

  // Auto-skip is opt-in per segment type and always announces itself, so a wrong
  // boundary costs a rewind rather than a silent jump.
  let lastAutoSkipped: SkipSegment | null = null;
  $effect(() => {
    if (!autoSkipTypes?.length || !session.skip_segments?.length) return;
    const t = displayTime;
    const hit = session.skip_segments.find(
      (s) => autoSkipTypes.includes(s.segment_type) && t >= s.start_seconds && t < s.end_seconds
    );
    if (!hit || hit === lastAutoSkipped) return;
    lastAutoSkipped = hit;
    untrack(() => handleSeek(hit.end_seconds));
    showMessage(`Skipped ${SEGMENT_LABELS[hit.segment_type] ?? 'segment'}`);
  });

  // Show Up Next overlay near end of playback (not for live)
  const UP_NEXT_THRESHOLD_SECONDS = 40;
  $effect(() => {
    if (!isLive && nextEpisode && !upNextDismissed && duration > 0) {
      const remaining = duration - displayTime;
      showUpNext = remaining <= UP_NEXT_THRESHOLD_SECONDS || videoEnded;
    }
  });

  $effect(() => {
    if (isLive || duration <= 0) return;
    const next = nextEpisode;
    const fraction = displayTime / duration;
    untrack(() => nextWarmer.consider(next, fraction));
  });

  function handleSkipSegment() {
    if (!activeSkipSegment) return;
    if (activeSkipSegment.segment_type === 'credits' && postCreditsScene) {
      handleSeek(postCreditsScene.start_seconds);
    } else if (activeSkipSegment.segment_type === 'credits') {
      const creditsEnd = activeSkipSegment.end_seconds;
      activeSkipSegment = null;
      // Credits that end before the file does have content after them (a
      // post-credits scene that wasn't tagged as its own segment). Seek past
      // the credits so it plays instead of closing the player over it. Only
      // treat as ended when the credits run to the end of the file — seeking
      // to the very end produces an empty HLS playlist and a transcoder retry.
      const END_EPSILON_SECONDS = 5;
      if (duration > 0 && creditsEnd < duration - END_EPSILON_SECONDS) {
        handleSeek(creditsEnd);
      } else if (nextEpisode) {
        handlePlayNextEpisode();
      } else {
        handleEnded();
      }
      return;
    } else {
      handleSeek(activeSkipSegment.end_seconds);
    }
    activeSkipSegment = null;
  }

  function formatSkipTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) seconds = 0;
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const pad = (n: number) => n.toString().padStart(2, '0');
    if (h > 0) {
      return `${pad(h)}:${pad(m)}:${pad(s)}`;
    }
    return `${pad(m)}:${pad(s)}`;
  }

  async function saveSkipEdit() {
    if (!editSegment) return;
    if (onSaveSkipSegment) {
      const seg = editSegment;
      try {
        await onSaveSkipSegment(seg, editStart, editEnd);
        seg.start_seconds = editStart;
        seg.end_seconds = editEnd;
        editingSkipSegment = false;
      } catch (e) {
        console.error('Failed to save skip segment:', e);
      }
      return;
    }
    const isMovie = activeCreationParams.media_type === 'movie';
    const fileId = activeCreationParams.file_id;

    try {
      const url = isMovie
        ? `/api/v1/admin/movies/files/${fileId}/skip-segments`
        : `/api/v1/admin/tv/episodes/files/${fileId}/skip-segments`;

      const body: Record<string, number> = {};
      if (editSegment.segment_type === 'intro') {
        body.intro_start = editStart;
        body.intro_end = editEnd;
      } else if (editSegment.segment_type === 'credits') {
        body.credits_start = editStart;
        body.credits_end = editEnd;
      }

      await fetch(url, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });

      editSegment.start_seconds = editStart;
      editSegment.end_seconds = editEnd;
      editingSkipSegment = false;
    } catch (e) {
      console.error('Failed to save skip segment:', e);
    }
  }

  async function deleteSkipSegment() {
    if (!editSegment) return;
    if (onDeleteSkipSegment) {
      try {
        await onDeleteSkipSegment(editSegment);
      } catch (e) {
        console.error('Failed to delete skip segment:', e);
        return;
      }
      editingSkipSegment = false;
      return;
    }
    editStart = 0;
    editEnd = 0;
    await saveSkipEdit();
    skipDismissed = true;
    editingSkipSegment = false;
  }

  async function switchToEpisode(next: NextEpisodeInfo, resumeAt = 0) {
    loading = true;
    error = null;

    // Save position for current episode, then tear down
    await savePosition();
    cleanup();

    try {
      await maybeDestroySession(session.session_id);

      const opened = await nextWarmer.open(next, activeCreationParams.capabilities);
      if (!opened) {
        throw new Error('No ready stream for this episode — try again shortly');
      }
      const { session: newSession, fileId: newFileId } = opened;

      // Update mutable state to point at the new episode
      activeMediaId = next.episode_id;
      activeCreationParams = { ...activeCreationParams, file_id: newFileId };
      activeMediaInfo = {
        title: next.show_title,
        subtitle: `S${next.season_identifier}E${next.episode_identifier}${next.title ? ` \u00b7 ${next.title}` : ''}`,
        seasonId: activeMediaInfo?.seasonId
      };
      session = newSession;
      duration = newSession.duration_seconds;

      // Reset playback tracking
      streamStarted = false;
      startHeld = false;
      userPaused = false;
      seekOffset = 0;
      currentTime = 0;
      buffered = 0;
      lastSavedPosition = Math.floor(resumeAt);
      markedWatched = false;
      videoEnded = false;
      showUpNext = false;
      upNextDismissed = false;
      nextEpisode = null;
      activeSkipSegment = null;
      skipDismissed = false;
      activeSubtitleId = null;
      activeAudioIndex = startingAudio(
        newSession.method,
        newSession.audio_tracks,
        newSession.preferred_audio_stream_index
      ).active;

      pendingResume = resumeAt > 5 ? resumeAt : 0;

      // Start playback (initPlayer applies volume normalization for new lufs)
      initPlayer();
      startHeartbeat();
      startPositionSaving();

      // Fetch next-next episode
      getNextEpisode(next.episode_id).then((info) => {
        nextEpisode = info;
      });

      // Update browser URL without navigation. Use SvelteKit's replaceState so
      // the router's own history state survives (a raw history.replaceState({})
      // wipes it and breaks back/forward).
      if (window.location.pathname.startsWith('/tv/episodes/')) {
        replaceState(`/tv/episodes/${next.episode_id}`, {});
      }
      onEpisodeChange?.(next.episode_id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to play episode';
      loading = false;
    }
  }

  async function handlePlayNextEpisode() {
    if (!nextEpisode) return;
    await switchToEpisode(nextEpisode);
  }

  async function handleSelectEpisode(episode: TvEpisode) {
    loading = true;
    try {
      // Try local files first, fall back to debrid
      const files = await getEpisodeFiles(episode.id);
      const seasonId = activeMediaInfo?.subtitle?.match(/S(\d+)/)?.[1] ?? '?';
      const info: NextEpisodeInfo = {
        episode_id: episode.id,
        title: episode.title,
        episode_identifier: episode.episode_identifier,
        season_identifier: seasonId,
        show_title: activeMediaInfo?.title ?? '',
        still_url: episode.still_url,
        file_id: files.length > 0 ? files[0].id : null
      };
      await switchToEpisode(info);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to switch episode';
      loading = false;
    }
  }

  function handleDismissUpNext() {
    upNextDismissed = true;
    showUpNext = false;
  }

  function considerHandoff(p: WatchProgress) {
    if (isLive || castingMode || !handoffScopeId) return;
    if (p.scope_id !== handoffScopeId || p.client_id === playbackClientId()) return;
    const savedAt = Date.parse(p.saved_at);
    if (!(savedAt > handoffSince)) return;
    // Playing here means watching here: that is someone else's session, not a
    // handoff. The other device's next save offers again once this one pauses.
    if (playing) return;
    if (
      p.media_id === activeMediaId &&
      Math.abs(p.position_seconds - effectiveTime) < HANDOFF_MIN_DELTA_SECONDS
    ) {
      handoffSince = savedAt;
      return;
    }
    handoff = p;
  }

  /**
   * Where this title stands across devices. `baseline` on open and on episode
   * change: whatever is already saved is what this player was opened against.
   * Without it (the event stream came back) a newer save is an offer.
   */
  async function syncHandoff(baseline: boolean) {
    const id = activeMediaId;
    const type = activeCreationParams.media_type;
    if (!id || (type !== 'movie' && type !== 'episode')) return;
    try {
      const r = await getPlaybackHandoff(type, id);
      if (id !== activeMediaId) return;
      handoffScopeId = r.scope_id;
      if (baseline) {
        if (r.latest) handoffSince = Math.max(handoffSince, Date.parse(r.latest.saved_at));
      } else if (r.latest) {
        considerHandoff(r.latest);
      }
    } catch (e) {
      console.warn('[Player] Handoff check failed:', e);
    }
  }

  function stopHandoffCountdown() {
    if (handoffStayTimer) clearInterval(handoffStayTimer);
    handoffStayTimer = null;
    handoffStayCountdown = null;
  }

  function handleHandoffStay() {
    if (handoff) handoffSince = Math.max(handoffSince, Date.parse(handoff.saved_at));
    stopHandoffCountdown();
    handoff = null;
  }

  async function handleHandoffJump() {
    const p = handoff;
    if (!p) return;
    stopHandoffCountdown();
    handoffSince = Math.max(handoffSince, Date.parse(p.saved_at));
    if (p.media_id === activeMediaId) {
      // Same title: the saved position is already the other device's, so there
      // is nothing to write — only somewhere to go.
      lastSavedPosition = p.position_seconds;
      handoff = null;
      userPaused = false;
      await handleSeek(p.position_seconds);
      videoEl?.play().catch((e) => console.warn('[Player] play() rejected:', e));
      return;
    }
    if (p.media_type !== 'episode' || !p.episode) {
      handoff = null;
      return;
    }
    // Kept open through the switch: switchToEpisode saves the outgoing episode's
    // position first, and that position is exactly the stale one.
    try {
      await switchToEpisode(
        {
          episode_id: p.media_id,
          title: p.episode.title,
          episode_identifier: p.episode.episode_identifier,
          season_identifier: p.episode.season_identifier,
          show_title: p.episode.show_title,
          still_url: p.episode.still_url,
          file_id: p.episode.file_id
        },
        p.watched ? 0 : p.position_seconds
      );
    } finally {
      handoff = null;
    }
  }

  function showControls() {
    controlsVisible = true;
    resetHideTimer();
  }

  function resetHideTimer() {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(
      () => {
        if (playing) controlsVisible = false;
      },
      isTouchDevice ? 4000 : 3000
    );
  }

  function handleMouseMove() {
    if (isTouchDevice) return;
    showControls();
  }

  // --- Touch gesture state (double-tap seek, single-tap controls toggle) ---
  let tapTimer: ReturnType<typeof setTimeout> | null = null;
  let lastTapTime = 0;
  let lastTapX = 0;
  let skipIndicator = $state<'rewind' | 'forward' | null>(null);
  let skipAccumulated = $state(0);
  let skipIndicatorTimer: ReturnType<typeof setTimeout> | null = null;
  let lastSkipDirection: 'rewind' | 'forward' | null = null;
  let lastSkipTime = 0;

  function isVideoArea(target: HTMLElement): boolean {
    return (
      target === videoEl ||
      target === containerEl ||
      target.classList.contains('loading-overlay') ||
      target.classList.contains('video-element')
    );
  }

  function handleContainerClick(e: MouseEvent) {
    if (isTouchDevice) return;
    const target = e.target as HTMLElement;
    if (isVideoArea(target)) {
      togglePlayPause();
    }
  }

  function handleTouchEnd(e: TouchEvent) {
    const target = e.target as HTMLElement;
    if (!isVideoArea(target)) return;

    const touch = e.changedTouches[0];
    const now = Date.now();
    const x = touch.clientX;

    if (now - lastTapTime < 300 && Math.abs(x - lastTapX) < 50) {
      // Double tap — seek
      if (tapTimer) {
        clearTimeout(tapTimer);
        tapTimer = null;
      }
      const rect = containerEl!.getBoundingClientRect();
      const isRight = x > rect.left + rect.width / 2;
      const direction: 'rewind' | 'forward' = isRight ? 'forward' : 'rewind';
      handleSeek(isRight ? Math.min(duration, displayTime + 10) : Math.max(0, displayTime - 10));
      showSkipIndicator(direction, true);
      showControls();
      lastTapTime = 0;
    } else {
      // Potential single tap — wait for possible double
      lastTapTime = now;
      lastTapX = x;
      tapTimer = setTimeout(() => {
        tapTimer = null;
        if (controlsVisible) {
          controlsVisible = false;
          if (hideTimer) clearTimeout(hideTimer);
        } else {
          showControls();
        }
      }, 300);
    }
  }

  function showSkipIndicator(direction: 'rewind' | 'forward', accumulate = false) {
    const now = Date.now();
    if (accumulate && direction === lastSkipDirection && now - lastSkipTime < 800) {
      skipAccumulated += 10;
    } else {
      skipAccumulated = 10;
    }
    lastSkipDirection = direction;
    lastSkipTime = now;
    skipIndicator = direction;
    if (skipIndicatorTimer) clearTimeout(skipIndicatorTimer);
    skipIndicatorTimer = setTimeout(() => {
      skipIndicator = null;
      skipAccumulated = 0;
      lastSkipDirection = null;
    }, 600);
  }

  function togglePlayPause() {
    if (!videoEl) return;
    console.log(
      '[Player] togglePlayPause — paused:',
      videoEl.paused,
      'readyState:',
      videoEl.readyState,
      'networkState:',
      videoEl.networkState,
      'error:',
      videoEl.error
    );
    if (videoEl.paused) {
      userPaused = false;
      videoEl.play().catch((e) => console.warn('[Player] play() rejected:', e));
    } else {
      userPaused = true;
      videoEl.pause();
    }
  }

  async function handleSeek(absoluteTime: number) {
    console.log(
      '[Seek] absoluteTime:',
      absoluteTime,
      'seekOffset:',
      seekOffset,
      'currentTime:',
      currentTime,
      'displayTime:',
      displayTime,
      'duration:',
      duration
    );
    if (!videoEl) return;

    // Direct play: always native seek (range requests handle it)
    if (session.method === 'direct_play') {
      videoEl.currentTime = absoluteTime;
      return;
    }

    // Compute local time within current HLS segment range
    const localTime = absoluteTime - seekOffset;

    // If within any buffered range, use native seek (instant, no server call).
    // Check all TimeRanges — discontinuities can create gaps between ranges.
    if (localTime >= 0 && videoEl.buffered.length > 0) {
      let inBuffer = false;
      for (let i = 0; i < videoEl.buffered.length; i++) {
        if (localTime >= videoEl.buffered.start(i) && localTime <= videoEl.buffered.end(i)) {
          inBuffer = true;
          break;
        }
      }
      if (inBuffer) {
        videoEl.currentTime = localTime;
        return;
      }
    }

    // Server-side seek: kill FFmpeg, restart from new position.
    // Guard against overlapping seeks — the seek bar fires oninput continuously
    // during drag, and each server-side seek kills/restarts FFmpeg.
    if (serverSeeking) {
      pendingSeekTime = absoluteTime;
      return;
    }

    await executeServerSeek(absoluteTime);
  }

  async function executeServerSeek(absoluteTime: number) {
    if (!videoEl) return;

    serverSeeking = true;
    pendingSeekTime = null;
    loading = true;
    error = null;
    try {
      const result = await seekPlayback(session.session_id, absoluteTime);

      // Full reload: new playlist starting from the seek target.
      // Destroy and recreate hls.js for a clean MediaSource — detach/reattach
      // leaves stale SourceBuffer state that can cause DEMUXER errors.
      // hls.js normalizes the playlist timeline so the first segment is
      // always at currentTime=0, regardless of PTS in the segments.
      // seekOffset maps currentTime back to absolute position for the UI.
      // Use the server's segment-aligned start_time, not the raw request time,
      // since FFmpeg restarts at segment boundaries (floor to segment_duration).
      seekOffset = result.start_time;
      currentTime = 0;
      buffered = 0;

      // Master playlist includes ?g=N on the stream.m3u8 URL server-side,
      // so browser can't serve a stale stream playlist from a previous seek.
      const playlistUrl = session.playlist_url! + `?g=${result.seek_generation}`;

      if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
      }

      // Clear the video element before creating a new hls.js instance —
      // leftover MediaSource references cause Format errors in Chrome MSE.
      videoEl.removeAttribute('src');
      videoEl.load();

      if (HlsClass && HlsClass.isSupported()) {
        // startHls registers the FRAG_BUFFERED listener that recalibrates
        // seekOffset from the real first PTS and jumps to localTarget.
        pendingCalibrationTarget = absoluteTime;
        pendingCalibrationServerOffset = result.sub_segment_offset;
        startHls(playlistUrl);
      } else if (videoEl.canPlayType('application/vnd.apple.mpegurl')) {
        // Safari native HLS: reports currentTime as absolute position in
        // the media, not relative to playlist start. Don't add seekOffset.
        seekOffset = 0;
        videoEl.src = playlistUrl;
        autoplay();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Seek failed';
      loading = false;
    } finally {
      serverSeeking = false;
    }

    // If another seek came in while we were busy, fire the latest one
    if (pendingSeekTime !== null) {
      const t = pendingSeekTime;
      pendingSeekTime = null;
      await handleSeek(t);
    }
  }

  function handleVolumeChange(v: number) {
    if (!videoEl) return;
    volume = v;
    videoEl.volume = v;
    if (v > 0 && muted) {
      muted = false;
      videoEl.muted = false;
    }
  }

  function handleMuteToggle() {
    if (!videoEl) return;
    muted = !muted;
    videoEl.muted = muted;
  }

  function handleSpeedChange(speed: number) {
    playbackSpeed = speed;
    if (videoEl) videoEl.playbackRate = speed;
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem('video:playbackSpeed', String(speed));
    }
  }

  function applyVideoNormalization() {
    if (!audioGainNode || session.lufs == null) return;
    if (normalizeVolume) {
      // EBU R128 / Apple HLS-style: aim for the target loudness, but cap the
      // boost so a quietly-mastered TV episode (-25 LUFS) doesn't get +11 dB
      // of unbounded gain that pushes peaks past 0 dBFS.
      // Without true-peak data per-file we use a conservative +6 dB ceiling
      // (≈2× linear). The downstream DynamicsCompressor catches anything that
      // still wants to clip on transients/intersample peaks.
      const targetLufs = -14;
      const maxBoostDb = 6;
      const requestedDb = targetLufs - session.lufs;
      const cappedDb = Math.min(requestedDb, maxBoostDb);
      audioGainNode.gain.value = Math.pow(10, cappedDb / 20);
    } else {
      audioGainNode.gain.value = 1.0;
    }
  }

  /**
   * Brick-wall limiter that sits between the gain node and the destination so
   * amplified content can't push the DAC past 0 dBFS. Tuned tight: -1 dBTP
   * threshold, hard knee, 20:1 ratio (effectively brickwall), 1 ms attack so
   * transients don't slip through, 50 ms release for natural recovery.
   */
  function createNormalizationLimiter(ctx: AudioContext): DynamicsCompressorNode {
    const limiter = ctx.createDynamicsCompressor();
    limiter.threshold.value = -1;
    limiter.knee.value = 0;
    limiter.ratio.value = 20;
    limiter.attack.value = 0.001;
    limiter.release.value = 0.05;
    return limiter;
  }

  // PiP ownership transfer: when true, cleanup/destroy skip video/HLS/session
  // because they've been handed off to the global videopip store.
  let enteredPip = false;

  // Called from onDestroy only — when the component is unmounting while a
  // PiP window is still live (e.g. the user navigated away). Hands the
  // active video element, HLS instance, AudioContext, session, and overlay
  // div to the global videopip store so playback survives the unmount.
  function transferToPipStore() {
    if (enteredPip || !videoEl || !containerEl || !sessionReady) return;
    enteredPip = true;
    enterPip({
      videoElement: videoEl,
      hlsInstance,
      overlayDiv: containerEl,
      session,
      creationParams: {
        media_type: activeCreationParams.media_type,
        file_id: activeCreationParams.file_id
      },
      mediaId: activeMediaId ?? '',
      seekOffset,
      audioContext
    });
    hlsInstance = null;
    audioContext = null;
    audioGainNode = null;
    audioLimiterNode = null;
  }

  function handlePipToggle() {
    if (!videoEl || !sessionReady) return;
    if (isPip) {
      document.exitPictureInPicture?.();
    } else {
      videoEl.requestPictureInPicture?.();
    }
  }

  function handleFullscreenToggle() {
    if (!containerEl) return;
    if (document.fullscreenElement) {
      screen.orientation?.unlock();
      document.exitFullscreen();
    } else {
      containerEl.requestFullscreen().then(() => {
        (screen.orientation as any)?.lock?.('landscape')?.catch?.(() => {});
      });
    }
  }

  async function handleSubtitleSelect(trackId: string | null) {
    const newTrack = trackId ? session.subtitles.find((s) => s.track_id === trackId) : null;
    const currentTrack = activeSubtitleId
      ? session.subtitles.find((s) => s.track_id === activeSubtitleId)
      : null;

    // Bitmap subtitle burn-in requires FFmpeg restart — use the switch-audio
    // endpoint with burn_in_subtitle_index to reconfigure in-place.
    if (newTrack?.requires_transcode || currentTrack?.requires_transcode) {
      const resumeTime = seekOffset + currentTime;
      loading = true;
      error = null;
      cleanup();

      try {
        // -1 disables burn-in, positive index enables it
        const burnIdx = newTrack?.requires_transcode ? newTrack.stream_index : -1;
        const updated = await switchAudioTrack(
          session.session_id,
          activeAudioIndex ?? session.audio_tracks[0]?.stream_index ?? 0,
          resumeTime,
          burnIdx
        );
        session = updated;
        activeSubtitleId = trackId;
        seekOffset = 0;
        initPlayer();
        if (resumeTime > 5) {
          await handleSeek(resumeTime);
        }
      } catch (e) {
        error = e instanceof Error ? e.message : 'Subtitle switch failed';
        loading = false;
      }
      return;
    }

    // Text subs: instant client-side switch
    activeSubtitleId = trackId;
    applySubtitles();
  }

  // True when it moved playback to the preferred track itself, which also
  // settles the method the probe would otherwise refine.
  function adoptStartingAudio(method: PlaybackMethod, preferred: number | undefined): boolean {
    const { active, switchTo } = startingAudio(method, session.audio_tracks, preferred);
    activeAudioIndex = active;
    if (switchTo === null || !sessionReady || audioSwitchedSession === session.session_id) {
      return false;
    }
    audioSwitchedSession = session.session_id;
    pendingMethodRefinement = null;
    void handleAudioTrackSelect(switchTo).then(watchDebridStart);
    return true;
  }

  async function handleAudioTrackSelect(streamIndex: number | null) {
    if (streamIndex === null || streamIndex === activeAudioIndex) return;

    const resumeTime = seekOffset + currentTime;
    loading = true;
    error = null;

    // Tear down current HLS player
    cleanup();

    try {
      // Switch audio track in-place — session ID is preserved, backend
      // kills FFmpeg and restarts with the new audio track.
      const updated = await switchAudioTrack(session.session_id, streamIndex, resumeTime);
      session = updated;
      activeAudioIndex = streamIndex;
      seekOffset = 0;

      // Reinitialize player with updated playlist
      initPlayer();

      // Seek to saved position (skip if near the start).
      // The backend already starts FFmpeg at resume_time, but we still
      // need to tell HLS.js where to begin in the playlist.
      if (resumeTime > 5) {
        await handleSeek(resumeTime);
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Audio track switch failed';
      loading = false;
    }
  }

  async function handleSubtitleDownloaded(track?: PlaybackSubtitleTrack) {
    showSubtitleSearch = false;
    if (track) {
      // Session-based download: the subtitle was saved to the session's segment
      // dir and is ready to serve. Just add it to the subtitle list and select it.
      session.subtitles = [...session.subtitles, track];
      activeSubtitleId = track.track_id;
      applySubtitles();
      return;
    }
    // Legacy path for entity-based downloads (non-session) — recreate session
    // to pick up newly downloaded external subtitle from DB.
    const resumeTime = seekOffset + currentTime;
    loading = true;
    error = null;
    cleanup();
    try {
      await maybeDestroySession(session.session_id);
      const newSession = await recreateSession({
        audio_track_index: activeAudioIndex ?? undefined
      });
      session = newSession;
      seekOffset = 0;
      if (!activeSubtitleId && newSession.subtitles.length > 0) {
        activeSubtitleId = newSession.subtitles[newSession.subtitles.length - 1].track_id;
      }
      initPlayer();
      if (resumeTime > 5) {
        await handleSeek(resumeTime);
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Session refresh failed';
      loading = false;
    }
  }

  function applySubtitles() {
    // Destroy existing JASSUB instance if any
    if (jassubInstance) {
      jassubInstance.destroy();
      jassubInstance = null;
    }

    // Reuse the existing programmatic VTT track across track switches —
    // creating a new TextTrack each time leaks 'kobbler-subs' tracks on
    // videoEl.textTracks (no removeTrack API exists). Just clear cues
    // and disable; rebuildVttTextTrack will repopulate and re-show.
    if (activeVttTextTrack) {
      activeVttTextTrack.mode = 'disabled';
      clearTrackCues(activeVttTextTrack);
    }
    vttParsedCues = [];
    vttAppliedOffset = null;

    if (!activeSubtitleId || !videoEl) return;

    const track = session.subtitles.find((s) => s.track_id === activeSubtitleId);
    if (!track) return;

    // Burn-in subtitles are baked into the video stream — nothing to render client-side
    if (track.format === 'burn_in') return;

    if (track.format === 'ass' && track.url) {
      initJassub(track.url);
    } else if (track.url) {
      loadVttTrack(track.url);
    }
  }

  async function loadVttTrack(url: string) {
    try {
      const res = await fetch(url);
      if (!res.ok) {
        console.error('[Subs] failed to fetch VTT:', res.status, url);
        return;
      }
      const vttText = await res.text();
      vttParsedCues = parseVtt(vttText);
      vttAppliedOffset = null;
      rebuildVttTextTrack();
    } catch (err) {
      console.error('[Subs] failed to load VTT track:', err);
    }
  }

  function clearTrackCues(track: TextTrack) {
    while (track.cues && track.cues.length > 0) {
      track.removeCue(track.cues[0]);
    }
  }

  /**
   * Build (or rebuild) a programmatic TextTrack from cached VTT cues.
   * Cue timestamps are shifted by -seekOffset so they align with
   * videoEl.currentTime, which hls.js normalizes to start from 0.
   */
  function rebuildVttTextTrack() {
    if (!videoEl || vttParsedCues.length === 0) return;

    // Skip rebuild if offset hasn't actually changed
    if (vttAppliedOffset === seekOffset && activeVttTextTrack) return;

    // Reuse the existing track if we have one (TextTrack API has no
    // removeTrack — creating a new one each time piles up disabled
    // 'kobbler-subs' entries on videoEl.textTracks).
    if (!activeVttTextTrack) {
      activeVttTextTrack = videoEl.addTextTrack('subtitles', 'kobbler-subs', 'und');
    } else {
      clearTrackCues(activeVttTextTrack);
    }

    const CueClass = window.VTTCue || (window as any).TextTrackCue;
    const offset = seekOffset;

    for (const cue of vttParsedCues) {
      const start = cue.start - offset;
      const end = cue.end - offset;
      // Only add cues that could appear in this segment's timeline
      if (end > 0) {
        activeVttTextTrack.addCue(new CueClass(Math.max(0, start), end, cue.text));
      }
    }

    activeVttTextTrack.mode = 'showing';
    vttAppliedOffset = offset;
  }

  async function initJassub(url: string) {
    if (!videoEl) return;
    try {
      const JASSUB = (await import('jassub')).default;
      jassubInstance = new JASSUB({
        video: videoEl,
        subUrl: url,
        workerUrl: '/jassub/jassub-worker.js',
        wasmUrl: '/jassub/jassub-worker.wasm'
      });
    } catch (err) {
      console.error('Failed to initialize JASSUB:', err);
    }
  }

  // Map a backend `X-Stream-Error` code to a user-facing message. Stable
  // codes come from `error_response()` in api/src/routes/admin/streaming.rs.
  function mapStreamError(code: string, fallback: string): string {
    switch (code) {
      case 'transcode_stalled':
        return 'Playback stalled \u2014 try again, or check the file for corruption';
      case 'startup_stalled':
        return "Couldn't start playback \u2014 the source may be stuck or unavailable";
      case 'source_disconnected':
        return 'Live source disconnected';
      case 'probe_failed':
        return "Couldn't read this file";
      case 'no_fallback_encoder':
        return 'No working video encoder available on the server';
      case 'transcode_failed':
        return fallback || 'Playback failed \u2014 the transcode crashed';
      default:
        return fallback || 'Playback failed';
    }
  }

  function handlePlaying() {
    if (streamStarted) return;
    streamStarted = true;
    stopStartWatch();
    if (isDebridSession && session.session_id) {
      reportStreamOutcome(session.session_id, { ok: true }).catch(() => {});
    }
  }

  function stopStartWatch() {
    startWatch?.abort();
    startWatch = null;
    startProgress = null;
  }

  function watchDebridStart() {
    stopStartWatch();
    if (!isDebridSession || streamStarted || startHeld || !session.session_id) return;
    const watch = new AbortController();
    startWatch = watch;
    const sessionId = session.session_id;
    void superviseStart({
      method: session.method,
      signal: watch.signal,
      getStatus: () => getPlaybackSessionStatus(sessionId),
      onProgress: (line) => {
        if (!watch.signal.aborted) startProgress = line;
      }
    }).then((outcome) => {
      if (watch.signal.aborted || outcome.kind === 'aborted') return;
      // Stalled while the server still had the file coming points at this
      // player's connection, not the release: reopen the same session once.
      if (
        outcome.kind === 'stalled' &&
        outcome.sourceHealthy &&
        reloadedStallSession !== sessionId
      ) {
        reloadedStallSession = sessionId;
        cleanup();
        initPlayer();
        startHeartbeat();
        startPositionSaving();
        watchDebridStart();
        return;
      }
      failPlayback(
        "That stream wouldn't start",
        outcome.kind === 'stalled' ? 'start_stalled' : 'session_lost'
      );
    });
  }

  function failPlayback(message: string, reason: string) {
    loading = false;
    if (isDebridSession && session.session_id) {
      const sessionId = session.session_id;
      const reported = streamStarted
        ? Promise.resolve()
        : reportStreamOutcome(sessionId, { ok: false, reason }).catch(() => {});
      if (
        onStreamFailed?.({
          sessionId,
          started: streamStarted,
          position: displayTime,
          reason,
          reported
        })
      ) {
        cleanup();
        return;
      }
    }
    error = message;
  }

  function returnToPending() {
    const resumeAt = displayTime;
    cleanup();
    // The <video> element unmounts with the session, so its audio graph can't be reused.
    audioContext?.close();
    audioContext = null;
    audioGainNode = null;
    audioLimiterNode = null;
    session = placeholderSession();
    sessionReady = false;
    initializedSessionId = null;
    error = null;
    loading = true;
    playing = false;
    streamStarted = false;
    startHeld = false;
    currentTime = 0;
    seekOffset = 0;
    recoveryTimestamps = [];
    // Carry the position into the next release so a reconnect resumes where it broke.
    if (resumeAt > 5) pendingResume = resumeAt;
  }

  function setupHlsErrorHandler(hls: Hls, playlistUrl: string) {
    if (!HlsClass) return;
    const Events = HlsClass.Events;
    const ErrorTypes = HlsClass.ErrorTypes;
    let mediaRecoveryAttempts = 0;
    const MAX_MEDIA_RECOVERIES = 3;
    // Tier-3 corruption fallback: only fire once per session attempt to avoid
    // looping if the re-encoded stream also fails (which would mean the
    // problem is downstream of the encoder).
    let decodeErrorRecoveryAttempted = false;

    let networkRetries = 0;
    const MAX_NETWORK_RETRIES = 5;

    hls.on(Events.ERROR, (_event, data) => {
      console.warn('hls.js error:', data.type, data.details, data.fatal, data.reason);

      if (!data.fatal) return;

      // Video already ended — don't retry a completed stream
      if (videoEnded) return;

      // HTTP 500 from the playlist or segment endpoint = terminal server-side
      // error (transcode crash, stall, source disconnect). The body is JSON
      // with `{ error, message }` — surface that to the user instead of
      // retrying or recreating the session.
      const responseCode = (data.response as { code?: number } | undefined)?.code;
      if (responseCode === 500) {
        const text = (data.response as { text?: string } | undefined)?.text ?? '';
        let code = '';
        let message = '';
        try {
          const parsed = JSON.parse(text) as { error?: string; message?: string };
          code = parsed.error ?? '';
          message = parsed.message ?? '';
        } catch {
          /* body wasn't JSON — fall through to generic message */
        }
        console.warn(`hls.js: terminal stream error code=${code} message=${message}`);
        hls.destroy();
        failPlayback(mapStreamError(code, message), code || 'server_error');
        return;
      }

      if (data.type === ErrorTypes.MEDIA_ERROR && mediaRecoveryAttempts < MAX_MEDIA_RECOVERIES) {
        mediaRecoveryAttempts++;
        console.warn(
          `hls.js: media error recovery attempt ${mediaRecoveryAttempts}/${MAX_MEDIA_RECOVERIES}`
        );
        hls.recoverMediaError();
      } else if (
        data.type === ErrorTypes.MEDIA_ERROR &&
        !decodeErrorRecoveryAttempted &&
        session.session_id
      ) {
        // MSE recovery exhausted for a media decode error — most often this is
        // source bitstream corruption that the browser HW decoder refuses to
        // conceal (FFmpeg's SW decoder would conceal it; Chrome's VAAPI
        // doesn't). Ask the server to flip into transcode mode + restart from
        // the keyframe before the failure point so we can resume on a clean
        // re-encoded stream. Reload the playlist with the new generation so
        // cached segments from the broken generation are bypassed.
        decodeErrorRecoveryAttempted = true;
        const at = videoEl?.currentTime ?? 0;
        console.warn(`hls.js: triggering Tier-3 decode-error recovery at t=${at.toFixed(2)}s`);
        reportPlaybackDecodeError(session.session_id, at)
          .then((result) => {
            const reloadUrl = `${playlistUrl}${playlistUrl.includes('?') ? '&' : '?'}g=${result.seek_generation}`;
            console.warn(`hls.js: reloading playlist after Tier-3 recovery: ${reloadUrl}`);
            mediaRecoveryAttempts = 0;
            hls.loadSource(reloadUrl);
          })
          .catch((err) => {
            console.warn('hls.js: Tier-3 decode-error recovery failed', err);
            failPlayback('Playback error — the stream cannot continue', 'decode_error');
          });
      } else if (data.type === ErrorTypes.NETWORK_ERROR) {
        // levelEmptyError / levelLoadError are expected when FFmpeg hasn't
        // produced segments yet (EVENT playlist is empty). Retry the playlist
        // instead of nuking the session — segments will appear shortly.
        // Exception: if we're near the end of the video, the empty playlist
        // means playback is done (e.g. skip credits seeked past all content).
        if (data.details === 'levelEmptyError' && duration > 0 && displayTime / duration >= 0.9) {
          handleEnded();
          return;
        }
        if (
          networkRetries < MAX_NETWORK_RETRIES &&
          (data.details === 'levelEmptyError' ||
            data.details === 'levelLoadError' ||
            data.details === 'manifestLoadError' ||
            data.details === 'fragLoadError' ||
            data.details === 'levelLoadTimeOut' ||
            data.details === 'fragLoadTimeOut')
        ) {
          networkRetries++;
          console.warn(`hls.js: retrying playlist (${networkRetries}/${MAX_NETWORK_RETRIES})`);
          setTimeout(() => {
            // manifestLoadError means the master playlist itself never parsed,
            // so hls.js has no levels to resume — `startLoad` is a no-op in
            // that state. Re-fetch the manifest via `loadSource`. For other
            // error kinds (fragment / level load), `startLoad` is correct.
            if (data.details === 'manifestLoadError') {
              hls.loadSource(playlistUrl);
            } else {
              hls.startLoad(-1);
            }
          }, 2000);
          return;
        }
        // Debrid sessions can't be recreated server-side — the same broken
        // session would loop forever. Surface the error immediately.
        if (isDebridSession) {
          failPlayback('Stream failed \u2014 try a different source', 'network_error');
          return;
        }
        recoverSession();
        return;
      } else {
        failPlayback('Playback error \u2014 the stream cannot continue', 'media_error');
      }
    });
  }

  /**
   * Create an hls.js instance, load the given playlist, attach to video element,
   * and auto-play once the manifest is parsed. Fire-and-forget — no Promise
   * wrapper, matching what works in the raw hls.js debug page.
   */
  function startHls(playlistUrl: string) {
    if (!videoEl || !HlsClass || !HlsClass.isSupported()) return;

    console.log(
      '[HLS] startHls',
      playlistUrl,
      'videoEl:',
      !!videoEl,
      'readyState:',
      videoEl.readyState
    );

    // Bounded retry policy: transient 5xx (e.g. a one-shot playlist hiccup)
    // recovers transparently, but terminal 500s (server signalled the stream
    // is dead) hit the fatal path quickly so the error UI fires instead of
    // hammering forever. Tuned for HLS live-edge tolerances.
    const loadPolicy = {
      default: {
        maxTimeToFirstByteMs: 10_000,
        maxLoadTimeMs: 20_000,
        timeoutRetry: { maxNumRetry: 2, retryDelayMs: 500, maxRetryDelayMs: 2000 },
        errorRetry: { maxNumRetry: 2, retryDelayMs: 500, maxRetryDelayMs: 2000 }
      }
    };
    const playlistLoadPolicy = {
      default: {
        maxTimeToFirstByteMs: 10_000,
        maxLoadTimeMs: 20_000,
        timeoutRetry: { maxNumRetry: 4, retryDelayMs: 500, maxRetryDelayMs: 2000 },
        errorRetry: { maxNumRetry: 4, retryDelayMs: 500, maxRetryDelayMs: 2000 }
      }
    };
    const fragLoadPolicy = {
      default: {
        maxTimeToFirstByteMs: 10_000,
        maxLoadTimeMs: 20_000,
        timeoutRetry: { maxNumRetry: 4, retryDelayMs: 500, maxRetryDelayMs: 2000 },
        errorRetry: { maxNumRetry: 6, retryDelayMs: 500, maxRetryDelayMs: 2000 }
      }
    };

    hlsInstance = new HlsClass(
      isLive
        ? {
            // IPTV segments are 4s, cut at the first source keyframe past that.
            // liveMaxLatencyDuration must exceed segment duration with margin —
            // setting it near the segment size makes hls.js seek forward on
            // every fragment to maintain the latency cap, which the user sees
            // as a skip at every fragment boundary.
            lowLatencyMode: false,
            liveSyncDuration: LIVE_SYNC_DURATION_S,
            liveMaxLatencyDuration: 45,
            manifestLoadingTimeOut: 20000,
            manifestLoadPolicy: loadPolicy,
            playlistLoadPolicy,
            fragLoadPolicy
          }
        : {
            // Buffer settings modeled on Jellyfin's configuration.
            // 30s forward buffer balances responsiveness with memory usage.
            maxBufferLength: 30,
            maxMaxBufferLength: 30,
            // Keep all played segments in memory for instant backward seeking
            // without re-fetching from the server.
            backBufferLength: Infinity,
            lowLatencyMode: false,
            // EVENT playlists (no ENDLIST) are treated as "live" by hls.js.
            // Default liveSyncDurationCount: 3 delays playback start until 3
            // segments exist. Zero holdback is safe — the server transcodes
            // far faster than realtime.
            liveSyncDuration: 0,
            manifestLoadingTimeOut: 20000,
            startPosition: 0,
            manifestLoadPolicy: loadPolicy,
            playlistLoadPolicy,
            fragLoadPolicy
          }
    );
    setupHlsErrorHandler(hlsInstance, playlistUrl);

    hlsInstance.on(
      HlsClass.Events.INIT_PTS_FOUND,
      (_: unknown, data: { initPTS: number; timescale: number }) => {
        if (!isLive && data.timescale > 0) {
          initPtsSeconds = data.initPTS / data.timescale;
        }
      }
    );

    // hls.js anchors currentTime=0 at the first fragment's PTS, but
    // seekOffset starts at session.start_time / seek-target — what the
    // SERVER asked for. FFmpeg lands on the nearest preceding keyframe,
    // typically up to ~1s earlier, so without recalibrating seekOffset
    // every cue is shifted by that delta. This one-shot listener fixes it
    // for both initial load and seek (handleSeek wires the jump-target).
    if (!isLive) {
      const Events = HlsClass.Events;
      const calibrate = () => {
        const hls = hlsInstance;
        if (!hls || !videoEl) return;
        hls.off(Events.FRAG_BUFFERED, calibrate);
        if (initPtsSeconds !== null) {
          if (Math.abs(seekOffset - initPtsSeconds) > 0.01) {
            seekOffset = initPtsSeconds;
          }
          if (pendingCalibrationTarget !== null) {
            const localTarget = pendingCalibrationTarget - initPtsSeconds;
            if (localTarget > 0.5) {
              videoEl.currentTime = localTarget;
            }
          }
          initPtsSeconds = null;
        } else if (pendingCalibrationServerOffset > 0.5) {
          videoEl.currentTime = pendingCalibrationServerOffset;
        }
        pendingCalibrationTarget = null;
        pendingCalibrationServerOffset = 0;
      };
      hlsInstance.on(Events.FRAG_BUFFERED, calibrate);
    }

    hlsInstance.on(HlsClass.Events.MANIFEST_LOADING, (_: unknown, d: { url: string }) => {
      console.log('[HLS] MANIFEST_LOADING', d.url);
    });
    hlsInstance.on(HlsClass.Events.MANIFEST_LOADED, () => {
      console.log('[HLS] MANIFEST_LOADED');
    });
    hlsInstance.on(HlsClass.Events.MANIFEST_PARSED, () => {
      console.log(
        '[HLS] MANIFEST_PARSED — calling play()',
        'readyState:',
        videoEl?.readyState,
        'paused:',
        videoEl?.paused,
        'error:',
        videoEl?.error
      );
      autoplay();

      // Resume from saved position (HLS only — direct play resumes in handleLoadedData)
      if (pendingResume) {
        const pos = pendingResume;
        pendingResume = 0;
        handleSeek(pos);
      }
    });
    hlsInstance.on(HlsClass.Events.FRAG_LOADED, () => {
      if (!videoEl) return;
      console.log(
        '[HLS] FRAG_LOADED — readyState:',
        videoEl.readyState,
        'paused:',
        videoEl.paused,
        'currentTime:',
        videoEl.currentTime,
        'duration:',
        videoEl.duration
      );
    });

    hlsInstance.loadSource(playlistUrl);
    hlsInstance.attachMedia(videoEl!);
    console.log('[HLS] loadSource + attachMedia done');
  }

  /**
   * Start playback for a freshly-initialised pipeline, unless the user has
   * paused — in which case leave it paused and let the buffer fill where they
   * left off.
   */
  function autoplay() {
    if (!videoEl) return;
    if (userPaused) {
      pauseElement();
      playing = false;
      holdStart();
      return;
    }
    videoEl.play().catch((e) => {
      console.warn('[Player] play() rejected:', e);
      if (e instanceof DOMException && e.name === 'NotAllowedError') holdStart();
    });
  }

  // Paused by the viewer or held for a gesture by the browser: not starting is
  // the viewer's call now, so it must not be read as a stalled stream.
  function holdStart() {
    startHeld = true;
    stopStartWatch();
  }

  function initPlayer() {
    if (!videoEl) {
      console.error('[Player] initPlayer: videoEl is null!');
      return;
    }

    const { method, playlist_url, direct_url } = session;
    console.log(
      '[Player] initPlayer — method:',
      method,
      'playlist_url:',
      playlist_url,
      'direct_url:',
      direct_url
    );

    if (method === 'direct_play' && direct_url) {
      videoEl.src = direct_url;
      autoplay();
    } else if (playlist_url) {
      // Prefer hls.js when available — Chrome/Chromium returns "maybe" for
      // canPlayType('application/vnd.apple.mpegurl') but can't actually play
      // fMP4 HLS natively, causing decode errors. Only use native HLS as a
      // fallback (Safari, where hls.js isn't needed).
      if (HlsClass && HlsClass.isSupported()) {
        startHls(playlist_url);
      } else if (videoEl.canPlayType('application/vnd.apple.mpegurl')) {
        videoEl.src = playlist_url;
        autoplay();
      } else {
        // Fallback: try native
        videoEl.src = playlist_url;
        autoplay();
      }
    }

    applySubtitles();
    applyVideoNormalization();
  }

  function handleTimeUpdate() {
    if (!videoEl) return;
    currentTime = videoEl.currentTime;
    updateBuffered();
    updateMediaSessionPosition();
    if (isLive) steerLiveLatency();
  }

  function steerLiveLatency() {
    if (!videoEl || !hlsInstance || videoEl.paused) return;
    const details = hlsInstance.latestLevelDetails;
    if (!details?.live) return;
    const latency = hlsInstance.latency;
    const rate = liveSyncRate({
      latency: latency > 0 ? latency : null,
      targetLatency: liveTargetLatency(
        LIVE_SYNC_DURATION_S,
        details.totalduration,
        details.targetduration
      ),
      targetDurationS: details.targetduration,
      currentRate: videoEl.playbackRate
    });
    if (rate !== videoEl.playbackRate) videoEl.playbackRate = rate;
  }

  function updateBuffered() {
    if (!videoEl || videoEl.buffered.length === 0) return;
    buffered = videoEl.buffered.end(videoEl.buffered.length - 1);
  }

  function handlePlay() {
    playing = true;
    userPaused = false;
    // Pressing play past an open offer is a soft "stay here" — soft, because a
    // remote's play button is pressed by reflex. The card stays up long enough
    // to change course; until it goes, nothing is saved.
    if (handoff && !handoffStayTimer) {
      handoffStayCountdown = HANDOFF_STAY_SECONDS;
      handoffStayTimer = setInterval(() => {
        handoffStayCountdown = (handoffStayCountdown ?? 1) - 1;
        if (handoffStayCountdown <= 0) handleHandoffStay();
      }, 1000);
    }
    programmaticPauses = 0;
    resetHideTimer();
    updateMediaSession();
  }

  function handlePause() {
    // A pause we didn't ask for is the user's — via the PiP window or native
    // controls, which bypass togglePlayPause. Reaching the end of the file
    // fires pause before ended; that's the file running out, not intent.
    if (programmaticPauses > 0) programmaticPauses--;
    else if (!videoEl?.ended) {
      userPaused = true;
      // A pause is where a viewer most often walks away to another device; the
      // periodic save could leave the server up to a full interval behind it.
      savePosition();
    }
    playing = false;
    controlsVisible = true;
    if (hideTimer) clearTimeout(hideTimer);
    updateMediaSession();
  }

  function handleDurationChange() {
    if (!videoEl || !isFinite(videoEl.duration) || videoEl.duration <= 0) return;

    if (session.method === 'direct_play') {
      // Direct play: video element always reports correct duration
      duration = videoEl.duration;
    }
    // For HLS: the server-provided duration (from ffprobe) is always correct.
    // videoEl.duration comes from the EVENT playlist which starts small and
    // grows as FFmpeg writes segments — using it would cause the progress bar
    // to show a tiny, growing duration instead of the real file length.
  }

  function handleLoadedData() {
    console.log(
      '[Player] loadeddata — readyState:',
      videoEl?.readyState,
      'duration:',
      videoEl?.duration,
      'paused:',
      videoEl?.paused
    );
    loading = false;

    // Resume from saved position (direct play only — HLS resumes in MANIFEST_PARSED)
    if (pendingResume && session.method === 'direct_play') {
      const pos = pendingResume;
      pendingResume = 0;
      handleSeek(pos);
    }
  }

  function handleVideoError() {
    if (!videoEl?.error) return;
    const code = videoEl.error.code;
    const msg = videoEl.error.message || '';
    console.error(
      '[Player] video error — code:',
      code,
      'msg:',
      msg,
      'readyState:',
      videoEl.readyState,
      'networkState:',
      videoEl.networkState
    );
    // MEDIA_ERR_ABORTED (1) is normal during cleanup/seek — ignore it
    if (code === MediaError.MEDIA_ERR_ABORTED) return;
    // For HLS/transcode sessions, hls.js handles all media errors internally
    // via recoverMediaError(). The video element fires onerror BEFORE hls.js
    // can attempt recovery — if we show error UI here, we preempt the
    // recovery and the user sees a false error. Only handle video element
    // errors for direct play where there's no hls.js to recover.
    if (session.method !== 'direct_play') return;
    if (isDebridSession && onStreamFailed) {
      failPlayback('Stream failed — try a different source', `media_error_${code}`);
      return;
    }
    recoverSession();
  }

  function handleWaiting() {
    loading = true;
  }

  function handleCanPlay() {
    loading = false;
  }

  function handleEnded() {
    if (isLive) {
      // Live streams shouldn't end — attempt recovery
      recoverSession();
      return;
    }
    // `playing = false` is UI state only; pause the element so audio/video stops
    pauseElement();
    loading = false;
    playing = false;
    videoEnded = true;

    // No next episode — close the player instead of leaving HLS to error-loop
    if (!nextEpisode) {
      handleClose();
    }
  }

  function handleFullscreenChange() {
    isFullscreen = !!document.fullscreenElement;
  }

  const isTouchDevice = typeof navigator !== 'undefined' && 'ontouchstart' in window;

  function handleOrientationChange() {
    if (!isTouchDevice || !containerEl) return;
    const landscape =
      screen.orientation?.type.startsWith('landscape') ?? window.innerWidth > window.innerHeight;
    if (landscape && !document.fullscreenElement) {
      containerEl.requestFullscreen().catch(() => {});
    } else if (!landscape && document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    // Do not capture keys if user is typing in an input
    const target = e.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;
    // In pending state, only allow Escape to close
    if (!sessionReady) {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
      return;
    }

    switch (e.key) {
      case ' ':
        e.preventDefault();
        togglePlayPause();
        break;
      case 'ArrowLeft':
        e.preventDefault();
        handleSeek(Math.max(0, displayTime - 10));
        showSkipIndicator('rewind');
        break;
      case 'ArrowRight':
        e.preventDefault();
        handleSeek(Math.min(duration, displayTime + 10));
        showSkipIndicator('forward');
        break;
      case 'f':
        e.preventDefault();
        handleFullscreenToggle();
        break;
      case 'm':
        e.preventDefault();
        handleMuteToggle();
        break;
      case 'Escape':
        e.preventDefault();
        handleClose();
        break;
    }

    showControls();
  }

  let sessionDestroyed = false;

  async function destroySessionOnce() {
    if (enteredPip || sessionDestroyed || castingMode || !sessionReady || !session.session_id)
      return;
    sessionDestroyed = true;
    try {
      await maybeDestroySession(session.session_id);
    } catch {
      // Session may have already expired or been removed
    }
  }

  async function handleClose() {
    if (!sessionReady) {
      onClose();
      return;
    }
    await savePosition();
    await destroySessionOnce();
    cleanup();
    onClose();
  }

  function handleRetry() {
    // Manual retry is a deliberate user action — give it a fresh recovery
    // budget. Clear the in-flight guard in case a prior attempt is wedged.
    recovering = false;
    recoverSession();
  }

  function cleanup() {
    stopHeartbeat();
    stopStartWatch();
    if (positionSaveInterval) clearInterval(positionSaveInterval);
    if (hideTimer) clearTimeout(hideTimer);
    if (loadingTimer) clearTimeout(loadingTimer);
    if (tapTimer) clearTimeout(tapTimer);
    if (skipIndicatorTimer) clearTimeout(skipIndicatorTimer);
    if (debridEventSource) {
      debridEventSource.close();
      debridEventSource = null;
    }
    // Subtitles don't survive PiP, always clean up
    if (jassubInstance) {
      jassubInstance.destroy();
      jassubInstance = null;
    }
    if (activeVttTextTrack) {
      activeVttTextTrack.mode = 'disabled';
      activeVttTextTrack = null;
    }
    vttParsedCues = [];
    vttAppliedOffset = null;
    // When entering PiP, video/HLS/audio have been transferred to the global
    // store — local refs are already null, so these blocks are skipped naturally.
    if (hlsInstance) {
      hlsInstance.destroy();
      hlsInstance = null;
    }
    // AudioContext + MediaElementSourceNode must outlive episode switches:
    // createMediaElementSource(videoEl) permanently captures the element's
    // audio output, and there's no way to undo it. Closing the context here
    // would leave the videoEl routed through a dead node, audio breaks, and
    // the video element stalls at currentTime=0 (manifest loads, segments
    // buffer, but playback never advances → bufferStalledError, black
    // screen). Teardown happens in onDestroy instead.
    if (videoEl && !enteredPip) {
      pauseElement();
      // hls.destroy() above already detached MSE (clears src + load via
      // bufferController.onMediaDetaching). Only reset manually for the
      // direct_play path where we set videoEl.src directly.
      if (videoEl.src) {
        videoEl.removeAttribute('src');
        videoEl.load();
      }
    }
  }

  // Skip detection finishes minutes after a file is ingested, and people press
  // play the moment the episode appears — so a session routinely fetches its
  // skip segments before they exist and then holds that empty list for the
  // whole watch. The debrid path has always pushed `skip_segments_ready` over
  // its own session SSE and keeps doing so; local files had no equivalent and
  // needed a reload. They get the same event over the library stream now.
  $effect(() => {
    const fileId = activeCreationParams.file_id;
    if (!fileId || isLive) return;

    const cb = onLibraryEvent(
      'skip_segments_ready',
      (data: { file_id: string; media_type: string; segments: SkipSegment[] }) => {
        // Detection is season-scoped, so this fires for every sibling episode
        // too — only adopt our own.
        if (data.file_id !== fileId || !Array.isArray(data.segments)) return;
        session.skip_segments = data.segments;
        // Any earlier dismissal was of a different (empty) set, so re-arm the
        // banner. Safe regardless of position: the effect that raises it only
        // matches a segment the playhead is currently inside, so a segment
        // already passed cannot pop up retroactively.
        skipDismissed = false;
      }
    );

    return () => offLibraryEvent('skip_segments_ready', cb);
  });

  // Cross-device handoff: learn this title's scope and baseline on open and on
  // every in-place episode change, then listen. Reconnects re-ask, because the
  // save that matters most is the one made while this tab was asleep.
  $effect(() => {
    if (isLive || !activeMediaId) return;
    untrack(() => void syncHandoff(true));
  });

  $effect(() => {
    if (isLive) return;
    const cb = onLibraryEvent('watch_progress', (p: WatchProgress) => considerHandoff(p));
    const rc = onReconnect(() => void syncHandoff(false));
    return () => {
      offLibraryEvent('watch_progress', cb);
      offReconnect(rc);
    };
  });

  // Debrid SSE: connect to status endpoint for background processing updates
  let debridEventSource: EventSource | null = null;

  $effect(() => {
    const isDebrid = activeCreationParams.file_id.startsWith('debrid-');
    const sessionId = session.session_id;

    if (!isDebrid || !sessionId) return;

    // Close any existing connection before opening a new one
    debridEventSource?.close();

    const es = new EventSource(`/api/v1/admin/stream/${sessionId}/status`);
    debridEventSource = es;

    es.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        switch (data.type) {
          case 'probe_complete':
            if (data.duration && data.duration > 0) {
              duration = data.duration;
            }
            if (data.subtitles) {
              session.subtitles = data.subtitles;
            }
            if (data.audio_tracks) {
              session.audio_tracks = data.audio_tracks;
            }
            if (
              data.audio_tracks &&
              adoptStartingAudio(
                data.method ?? session.method,
                data.preferred_audio_stream_index ?? session.preferred_audio_stream_index
              )
            ) {
              break;
            }
            // The initial session response carries an OPTIMISTIC method based
            // on the client's advertised container capabilities. The probe
            // refines the decision once real codecs are known. Stash the
            // refined method in reactive state — an effect below applies it
            // once the session is fully initialised. Doing it via an effect
            // (instead of applying inline here) is race-proof: if this SSE
            // event arrives before the initial sessionResponse $effect has
            // run `session = sr`, that $effect would clobber our in-place
            // mutation. The effect re-runs after the clobber and re-applies.
            if (
              data.method === 'direct_play' ||
              data.method === 'remux' ||
              data.method === 'audio_transcode' ||
              data.method === 'video_transcode'
            ) {
              pendingMethodRefinement = data.method;
            }
            break;
          case 'skip_segments_ready':
            if (data.segments) {
              session.skip_segments = data.segments;
            }
            break;
          case 'thumbnails_ready':
            if (data.thumbnails_url) {
              session.thumbnails_url = data.thumbnails_url;
            }
            break;
          case 'download_complete':
            // Cache file is ready — no action needed, stream_play will use it
            break;
        }
      } catch {
        // Ignore malformed SSE data
      }
    };

    es.onerror = () => {
      // SSE reconnects automatically; close after persistent failures
      // (EventSource handles reconnection internally)
    };

    return () => {
      es.close();
      debridEventSource = null;
    };
  });

  /**
   * Apply a probe-refined `method` (from SSE `probe_complete`) to the live
   * session. Idempotent: if `session.method` already matches, does nothing.
   * Guarded on `sessionReady` and an assigned `session.session_id` so it
   * no-ops until the initial session response has populated `session`.
   *
   * Running this from a $effect (rather than inline in the SSE handler)
   * keeps it race-proof against the `session = sr` assignment in the
   * initial sessionResponse $effect: if that runs AFTER our inline
   * mutation it would overwrite the refined method, but the $effect below
   * re-runs on any session mutation and re-applies.
   */
  function applyMethodRefinement(newMethod: PlaybackMethod) {
    if (!session.session_id || session.method === newMethod) return;
    const newPlaylistUrl =
      newMethod === 'direct_play'
        ? null
        : `/api/v1/admin/playback/sessions/${session.session_id}/master.m3u8`;
    const newDirectUrl =
      newMethod === 'direct_play' ? `/api/v1/admin/stream/${session.session_id}/play` : null;
    console.log('[Player] method refined by probe:', session.method, '->', newMethod);
    if (hlsInstance) {
      hlsInstance.destroy();
      hlsInstance = null;
    }
    if (videoEl) {
      pauseElement();
      videoEl.removeAttribute('src');
      videoEl.load();
    }
    session.method = newMethod;
    session.playlist_url = newPlaylistUrl;
    session.direct_url = newDirectUrl;
    if (videoEl && sessionReady) {
      initPlayer();
    }
  }

  $effect(() => {
    const refined = pendingMethodRefinement;
    // Read reactive deps so the effect re-runs on any session reset.
    const sid = session.session_id;
    const currentMethod = session.method;
    if (!refined || !sessionReady || !sid) return;
    if (currentMethod === refined) return;
    applyMethodRefinement(refined);
  });

  onMount(async () => {
    // Tear down any existing PiP session — new player is taking over
    if (isPipActive()) {
      cleanupPip();
    }

    console.log(
      '[Player] onMount — videoEl:',
      !!videoEl,
      'session.method:',
      session.method,
      'pending:',
      !sessionReady
    );
    registerOverlay(overlayId, handleClose);
    window.addEventListener('beforeunload', releaseNextWarm);
    document.addEventListener('keydown', handleKeydown);
    document.addEventListener('fullscreenchange', handleFullscreenChange);
    setupMediaSessionHandlers();
    updateMediaSession();
    if (isTouchDevice && screen.orientation) {
      screen.orientation.addEventListener('change', handleOrientationChange);
    }

    // Listen for Cast connection state changes
    const unsubState = onStateChange((connected, _deviceName, resumed) => {
      if (connected && !castingMode && !resumed) {
        transferToCast();
      } else if (!connected && castingMode) {
        transferFromCast();
      }
    });
    // Track Cast position for resume on disconnect
    const unsubMsg = onCastMessage((msg) => {
      if (msg.type === 'POSITION_UPDATE' && castingMode) {
        lastCastPosition = msg.payload.currentTime;
      }
    });
    castUnsubscribe = () => {
      unsubState();
      unsubMsg();
    };

    // Load hls.js once, before initializing player
    if (!HlsClass) {
      console.log('[Player] loading hls.js...');
      const HlsModule = await import('hls.js');
      HlsClass = HlsModule.default;
      console.log('[Player] hls.js loaded, version:', HlsClass.version);
    }

    // If pending (no session yet), defer initialization — $effect handles it when session arrives
    if (!sessionReady) return;

    videoEl!.playbackRate = playbackSpeed;

    initPlayer();
    startHeartbeat();
    watchDebridStart();
    resetHideTimer();

    // Set up Web Audio for volume normalization
    if (session.lufs != null && videoEl) {
      try {
        audioContext = new AudioContext();
        const source = audioContext.createMediaElementSource(videoEl);
        audioGainNode = audioContext.createGain();
        audioLimiterNode = createNormalizationLimiter(audioContext);
        source.connect(audioGainNode);
        audioGainNode.connect(audioLimiterNode);
        audioLimiterNode.connect(audioContext.destination);
        applyVideoNormalization();
      } catch (e) {
        console.warn('Web Audio setup failed:', e);
      }
    }

    if (activeCreationParams.media_type === 'episode' && activeMediaId) {
      getNextEpisode(activeMediaId).then((info) => {
        nextEpisode = info;
      });
      if (activeMediaInfo?.seasonId) {
        getSeason(activeMediaInfo.seasonId).then((season) => {
          seasonEpisodes = season.episodes;
        });
      }
    }

    startPositionSaving();
  });

  onDestroy(() => {
    unmounted = true;
    nextWarmer.release();
    keep.dispose();
    window.removeEventListener('beforeunload', releaseNextWarm);
    unregisterOverlay(overlayId);
    document.removeEventListener('keydown', handleKeydown);
    document.removeEventListener('fullscreenchange', handleFullscreenChange);
    if (screen.orientation) {
      screen.orientation.removeEventListener('change', handleOrientationChange);
    }
    stopHandoffCountdown();
    // Clean up Cast listener
    castUnsubscribe?.();
    if (castPlaybackToken) {
      revokePlaybackToken(castPlaybackToken).catch(() => {});
    }
    // If the component is unmounting while PiP is still active (e.g. user
    // navigated away mid-PiP), hand the live resources to the global videopip
    // store so playback survives. Setting `enteredPip` short-circuits the
    // session/HLS/audio teardown below and the portal action's removal.
    if (
      !castingMode &&
      !sessionDestroyed &&
      sessionReady &&
      videoEl &&
      typeof document !== 'undefined' &&
      document.pictureInPictureElement === videoEl
    ) {
      transferToPipStore();
    }
    // Destroy backend session if not already destroyed (e.g. navigation away without explicit close)
    if (!castingMode) {
      destroySessionOnce();
    }
    cleanup();
    // Close AudioContext only at true unmount — kept alive across episode
    // switches because MediaElementSourceNode is permanent for the videoEl.
    // Skipped when transferred to the PiP store (which now owns it).
    if (audioContext && !enteredPip) {
      audioContext.close();
      audioContext = null;
      audioGainNode = null;
      audioLimiterNode = null;
    }
  });
</script>

<div
  class="video-player-overlay"
  class:pip-active={isPip}
  use:portal
  bind:this={containerEl}
  onmousemove={handleMouseMove}
  onclick={handleContainerClick}
  ontouchend={handleTouchEnd}
  onkeydown={() => {}}
  role="dialog"
  aria-label="Video player"
  tabindex="-1"
>
  {#if !sessionReady}
    <StartShell
      title={activeMediaInfo?.title ?? ''}
      subtitle={activeMediaInfo?.subtitle}
      artUrl={startArt}
      status={pendingStatus ?? 'Starting…'}
      caching={pendingCaching}
      onCancel={onClose}
      onChooseRelease={onPickStream}
    />
  {:else}
    {#if showStartShell}
      <StartShell
        title={activeMediaInfo?.title ?? ''}
        subtitle={activeMediaInfo?.subtitle}
        artUrl={startArt}
        status={startProgress ?? 'Starting…'}
        onCancel={handleClose}
      />
    {:else if loading}
      <div class="loading-overlay">
        <span class="material-icons-outlined spinner-icon">refresh</span>
        {#if loadingMessage}
          <span class="loading-text">{loadingMessage}</span>
        {/if}
      </div>
    {/if}

    {#if error}
      <div class="error-overlay">
        <span class="material-icons-outlined error-icon">error_outline</span>
        <p class="error-text">{error}</p>
        <div class="error-actions">
          <button class="error-btn" type="button" onclick={handleRetry}>Retry</button>
          <button class="error-btn secondary" type="button" onclick={handleClose}>Close</button>
        </div>
      </div>
    {/if}
  {/if}

  {#if sessionReady && castingMode}
    <div class="casting-overlay">
      <span class="material-icons-outlined casting-icon">cast_connected</span>
      <p class="casting-text">Casting to {castState.deviceName ?? 'device'}</p>
      <button class="casting-stop-btn" type="button" onclick={() => endCastSession()}>
        Stop casting
      </button>
    </div>
  {/if}

  {#if sessionReady}
    <video
      bind:this={videoEl}
      class="video-element {castingMode ? 'hidden' : ''}"
      style="object-fit: {effectiveObjectFit};"
      ontimeupdate={handleTimeUpdate}
      onplay={handlePlay}
      onplaying={handlePlaying}
      onpause={handlePause}
      ondurationchange={handleDurationChange}
      onloadeddata={handleLoadedData}
      onloadedmetadata={handleLoadedMetadata}
      onwaiting={handleWaiting}
      oncanplay={handleCanPlay}
      onended={handleEnded}
      onerror={handleVideoError}
      playsinline
    ></video>

    {#if skipIndicator}
      <div
        class="skip-indicator"
        class:left={skipIndicator === 'rewind'}
        class:right={skipIndicator === 'forward'}
      >
        <div class="skip-indicator-content">
          <span class="material-icons-outlined skip-indicator-icon">
            {skipIndicator === 'forward' ? 'fast_forward' : 'fast_rewind'}
          </span>
          <span class="skip-indicator-text">{skipAccumulated || 10} seconds</span>
        </div>
      </div>
    {/if}

    {#if activeSkipSegment && !handoff}
      <div class="skip-segment-group">
        <button class="skip-segment-btn" type="button" onclick={handleSkipSegment}>
          {activeSkipSegment.segment_type === 'intro'
            ? 'Skip Intro'
            : activeSkipSegment.segment_type === 'recap'
              ? 'Skip Recap'
              : activeSkipSegment.segment_type === 'ad_break'
                ? 'Skip Ads'
                : postCreditsScene
                  ? 'Skip to Post-Credits Scene'
                  : 'Skip Credits'}
        </button>
        <button
          class="skip-edit-btn"
          type="button"
          onclick={() => {
            if (activeSkipSegment) openSkipEditor(activeSkipSegment);
          }}
          title="Edit segment boundaries"
        >
          <span class="material-icons-outlined" style="font-size: 16px">edit</span>
        </button>
      </div>
    {/if}

    {#if editingSkipSegment && editSegment}
      <div class="skip-editor">
        <div class="skip-editor-header">
          Edit {editSegment.segment_type === 'ad_break'
            ? 'Ad Break'
            : editSegment.segment_type === 'intro'
              ? 'Intro'
              : 'Credits'} Boundaries
        </div>
        <div class="skip-editor-row">
          <span class="skip-editor-label">Start</span>
          <button type="button" onclick={() => (editStart = Math.max(0, editStart - 1))}>-1s</button
          >
          <span class="time-value">{formatSkipTime(editStart)}</span>
          <button type="button" onclick={() => (editStart += 1)}>+1s</button>
        </div>
        <div class="skip-editor-row">
          <span class="skip-editor-label">End</span>
          <button type="button" onclick={() => (editEnd = Math.max(editStart + 1, editEnd - 1))}
            >-1s</button
          >
          <span class="time-value">{formatSkipTime(editEnd)}</span>
          <button type="button" onclick={() => (editEnd += 1)}>+1s</button>
        </div>
        <div class="skip-editor-actions">
          <button type="button" class="save-btn" onclick={saveSkipEdit}>Save</button>
          <button type="button" class="delete-btn" onclick={deleteSkipSegment}
            >{editSegment.segment_type === 'ad_break'
              ? 'Not an ad break'
              : 'Not an intro/credits'}</button
          >
          <button type="button" class="cancel-btn" onclick={() => (editingSkipSegment = false)}
            >Cancel</button
          >
        </div>
      </div>
    {/if}

    {#if handoff}
      <HandoffPrompt
        progress={handoff}
        otherEpisode={handoff.media_id !== activeMediaId}
        stayCountdown={handoffStayCountdown}
        onJump={handleHandoffJump}
        onStay={handleHandoffStay}
      />
    {:else if nextEpisode && showUpNext}
      <UpNextOverlay
        {nextEpisode}
        visible={showUpNext}
        onPlayNow={handlePlayNextEpisode}
        onCancel={handleDismissUpNext}
        autoPlay={videoEnded}
      />
    {/if}

    {#if bottomPanel}
      <div class="bottom-panel" class:visible={controlsVisible || editingSkipSegment}>
        {@render bottomPanel()}
      </div>
    {/if}

    <PlayerControls
      {playing}
      currentTime={displayTime}
      {duration}
      buffered={displayBuffered}
      {volume}
      {muted}
      {isFullscreen}
      {isLive}
      visible={controlsVisible}
      subtitles={session.subtitles}
      {activeSubtitleId}
      audioTracks={session.audio_tracks}
      {activeAudioIndex}
      onPlayPause={togglePlayPause}
      onSeek={handleSeek}
      onVolumeChange={handleVolumeChange}
      onMuteToggle={handleMuteToggle}
      onFullscreenToggle={handleFullscreenToggle}
      onSubtitleSelect={handleSubtitleSelect}
      onAudioTrackSelect={handleAudioTrackSelect}
      {playbackSpeed}
      onSpeedChange={handleSpeedChange}
      {isPip}
      onPipToggle={handlePipToggle}
      {pipSupported}
      {aspectMode}
      onAspectModeCycle={cycleAspectMode}
      onSearchSubtitles={isLive ? undefined : () => (showSubtitleSearch = true)}
      onClose={handleClose}
      mediaTitle={activeMediaInfo?.title}
      mediaSubtitle={activeMediaInfo?.subtitle}
      seasonEpisodes={activeCreationParams.media_type === 'episode' ? seasonEpisodes : []}
      currentEpisodeId={activeMediaId}
      onSelectEpisode={handleSelectEpisode}
      thumbnailsVttUrl={isLive ? null : (session.thumbnails_url ?? null)}
      chapters={isLive ? [] : (session.chapters ?? [])}
      skipSegments={isLive ? [] : (session.skip_segments ?? [])}
      onEditSkipSegment={openSkipEditor}
      onSwitchStream={isDebridSession ? onSwitchStream : undefined}
      keep={isDebridSession && session.session_id
        ? {
            label: keep.label,
            busy: keep.busy,
            saved: keep.saved,
            onKeep: () => void keep.keep(session.session_id)
          }
        : undefined}
      {isTouchDevice}
      onInteraction={resetHideTimer}
    />
  {/if}
</div>

{#if showSubtitleSearch && !isLive && activeMediaId}
  <SubtitleSearchModal
    mediaType={activeCreationParams.media_type as 'movie' | 'episode'}
    entityId={activeMediaId}
    fileId={activeCreationParams.file_id}
    sessionId={session.session_id}
    onClose={() => (showSubtitleSearch = false)}
    onDownloaded={handleSubtitleDownloaded}
  />
{/if}

<style>
  .video-player-overlay {
    position: fixed;
    inset: -1px;
    background: #000;
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: none;
  }

  .video-player-overlay:hover {
    cursor: default;
  }

  /* While PiP is active the OS owns the video surface — hide the page-side
     overlay so the user sees the page underneath. The video element stays in
     the DOM so PiP rendering continues against this same source. */
  .video-player-overlay.pip-active {
    display: none;
  }

  .video-element {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .video-element::cue {
    font-size: 1.1em;
    background: rgba(0, 0, 0, 0.7);
    color: #fff;
    border-radius: 2px;
    padding: 2px 6px;
  }

  .video-element.hidden {
    display: none;
  }

  .casting-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    z-index: 2;
  }

  .casting-icon {
    font-size: 64px;
    color: #7c5cfc;
  }

  .casting-text {
    font-size: 18px;
    color: rgba(255, 255, 255, 0.8);
    margin: 0;
  }

  .casting-stop-btn {
    padding: 8px 24px;
    font-size: 14px;
    color: #fff;
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.2);
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.2s;
  }

  .casting-stop-btn:hover {
    background: rgba(255, 255, 255, 0.2);
  }

  .loading-overlay {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 3;
    pointer-events: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .spinner-icon {
    font-size: 48px;
    color: rgba(255, 255, 255, 0.8);
    animation: spin 1s linear infinite;
  }

  .loading-text {
    color: rgba(255, 255, 255, 0.7);
    font-size: 13px;
  }

  .error-overlay {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 4;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    text-align: center;
  }

  .error-icon {
    font-size: 48px;
    color: #e57373;
  }

  .error-text {
    color: rgba(255, 255, 255, 0.9);
    font-size: 14px;
    margin: 0;
    max-width: 400px;
  }

  .error-actions {
    display: flex;
    gap: 12px;
  }

  .error-btn {
    padding: 8px 20px;
    border-radius: 6px;
    border: none;
    background: var(--accent, #e8a87c);
    color: #000;
    font-size: 14px;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .error-btn:hover {
    opacity: 0.85;
  }

  .error-btn.secondary {
    background: rgba(255, 255, 255, 0.15);
    color: #fff;
  }

  .bottom-panel {
    position: absolute;
    left: calc(28px + env(safe-area-inset-left));
    right: calc(28px + env(safe-area-inset-right));
    bottom: calc(150px + env(safe-area-inset-bottom));
    z-index: 9;
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s ease;
  }

  .bottom-panel.visible {
    opacity: 1;
    pointer-events: auto;
  }

  .skip-segment-group {
    position: absolute;
    bottom: calc(100px + env(safe-area-inset-bottom));
    right: calc(24px + env(safe-area-inset-right));
    z-index: 10;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .skip-segment-btn {
    padding: 8px 24px;
    border: 1px solid rgba(255, 255, 255, 0.4);
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.7);
    color: white;
    font-size: 16px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.15s,
      transform 0.15s;
    backdrop-filter: blur(8px);
  }

  .skip-segment-btn:hover {
    background: rgba(255, 255, 255, 0.15);
    transform: scale(1.02);
  }

  .skip-edit-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.3);
    background: rgba(0, 0, 0, 0.6);
    color: rgba(255, 255, 255, 0.7);
    cursor: pointer;
    backdrop-filter: blur(8px);
  }

  .skip-edit-btn:hover {
    color: white;
    background: rgba(0, 0, 0, 0.8);
  }

  .skip-editor {
    position: absolute;
    bottom: calc(140px + env(safe-area-inset-bottom));
    right: calc(24px + env(safe-area-inset-right));
    z-index: 11;
    background: rgba(0, 0, 0, 0.9);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 8px;
    padding: 12px;
    backdrop-filter: blur(12px);
    min-width: 260px;
  }

  .skip-editor-header {
    font-size: 13px;
    font-weight: 600;
    color: white;
    margin-bottom: 10px;
  }

  .skip-editor-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }

  .skip-editor-label {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
    min-width: 35px;
  }

  .skip-editor-row button {
    padding: 2px 8px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    border-radius: 4px;
    background: transparent;
    color: rgba(255, 255, 255, 0.8);
    font-size: 12px;
    cursor: pointer;
  }

  .skip-editor-row button:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .time-value {
    font-variant-numeric: tabular-nums;
    font-size: 13px;
    color: white;
    min-width: 55px;
    text-align: center;
  }

  .skip-editor-actions {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }

  .skip-editor-actions button {
    padding: 4px 10px;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    cursor: pointer;
  }

  .skip-editor-actions .save-btn {
    background: var(--accent, #818cf8);
    color: white;
  }

  .skip-editor-actions .delete-btn {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .skip-editor-actions .cancel-btn {
    background: transparent;
    color: rgba(255, 255, 255, 0.6);
  }

  /* Skip indicator (double-tap / arrow key seek feedback) */
  .skip-indicator {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 40%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.08);
    z-index: 5;
    pointer-events: none;
    animation: skip-fade 0.6s ease-out forwards;
  }

  .skip-indicator.left {
    left: 0;
    border-radius: 0 50% 50% 0;
  }

  .skip-indicator.right {
    right: 0;
    border-radius: 50% 0 0 50%;
  }

  .skip-indicator-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    color: white;
  }

  .skip-indicator-icon {
    font-size: 40px;
  }

  .skip-indicator-text {
    font-size: 13px;
    font-weight: 500;
  }

  @keyframes skip-fade {
    0% {
      opacity: 1;
    }
    70% {
      opacity: 1;
    }
    100% {
      opacity: 0;
    }
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
