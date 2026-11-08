/**
 * Kobbler Cast Receiver
 *
 * Custom CAF v3 Web Receiver with:
 * - HLS playback via Cast media player
 * - Generation-based seek (Kobbler's HLS model)
 * - Skip segment overlays (intro, credits, recap)
 * - Thumbnail previews during seek scrub
 * - Chapter display
 * - Heartbeat and session lifecycle
 * - Google TV D-pad navigation support
 */

const NAMESPACE = 'urn:x-cast:com.kobbler.cast';

// ── State ──────────────────────────────────────────────────────

function freshSession() {
  return {
    baseUrl: '',
    token: '',
    sessionId: '',
    mediaId: null,
    mediaType: null,
    playlistUrl: null,
    directUrl: null,
    method: 'hls',
    duration: 0,
    startOffset: 0,
    subtitles: [],
    audioTracks: [],
    thumbnailsUrl: null,
    chapters: [],
    skipSegments: [],
    capabilities: null,
    metadata: {},
    _isDebrid: false
  };
}

let sessionState = freshSession();

let thumbnailEntries = [];
let heartbeatInterval = null;
let skipCheckInterval = null;
let chapterCheckInterval = null;
let positionInterval = null;
let currentChapterIndex = -1;
let seekGeneration = 0;

// Audiobook state
let audiobookState = null; // null when not in audiobook mode
let audiobookFileIndex = 0;

let ttsState = null;

// Token refresh tracking
let tokenCreatedAt = Date.now();
const TOKEN_REFRESH_AFTER_MS = 20 * 60 * 60 * 1000; // Refresh after 20 hours

// ── DOM refs ───────────────────────────────────────────────────

const skipOverlay = document.getElementById('skip-overlay');
const skipBtn = document.getElementById('skip-btn');
const thumbnailPreview = document.getElementById('thumbnail-preview');
const chapterDisplay = document.getElementById('chapter-display');
const artEl = document.getElementById('art');
const idleEl = document.getElementById('idle');
const hudEl = document.getElementById('hud');
const hudTitle = document.getElementById('hud-title');
const hudSubtitle = document.getElementById('hud-subtitle');
const hudState = document.getElementById('hud-state');
const hudElapsed = document.getElementById('hud-elapsed');
const hudTotal = document.getElementById('hud-total');
const hudFill = document.getElementById('hud-fill');

// ── CAF Setup ──────────────────────────────────────────────────

const context = cast.framework.CastReceiverContext.getInstance();
const playerManager = context.getPlayerManager();
// Our own element rather than <cast-media-player>, whose built-in progress bar counts from
// the start of the playlist: a transcode made from a resume would read 0:00 at minute 23.
playerManager.setMediaElement(document.getElementById('media'));

// Custom namespace for sender communication
context.addCustomMessageListener(NAMESPACE, (event) => {
  handleSenderMessage(event.data);
});

// ── Message Handling ───────────────────────────────────────────

function handleSenderMessage(msg) {
  switch (msg.type) {
    case 'INIT_SESSION':
      initSession(msg.payload);
      break;
    case 'INIT_AUDIOBOOK':
      initAudiobook(msg.payload);
      break;
    case 'INIT_TTS':
      initTts(msg.payload);
      break;
    case 'TTS_STOP':
      stopTts();
      break;
    case 'AUDIOBOOK_SEEK':
      seekAudiobook(msg.payload.fileIndex, msg.payload.time);
      break;
    case 'AUDIOBOOK_RATE':
      if (audiobookState) {
        audiobookState.playbackRate = msg.payload.rate;
        playerManager.setPlaybackRate(msg.payload.rate);
      }
      break;
    case 'SUBTITLE_CHANGE':
      handleSubtitleChange(msg.payload);
      break;
    case 'AUDIO_SWITCH':
      handleAudioSwitch(msg.payload);
      break;
    case 'ENRICHMENT_UPDATE':
      handleEnrichment(msg.payload);
      break;
    case 'TOKEN_REFRESH':
      sessionState.token = msg.payload.token;
      break;
    case 'STATUS_REQUEST':
      sendStatus();
      break;
    case 'CAPABILITIES_REQUEST':
      sendCapabilities();
      break;
    case 'TRACKS_REQUEST':
      sendTracks();
      break;
  }
}

function sendToBroadcast(msg) {
  context.sendCustomMessage(NAMESPACE, undefined, msg);
}

// <cast-media-player> keeps its media element in a shadow root, out of reach of
// document.querySelector, so the player manager is the only reliable source. A
// transcode's playlist starts where it was made from, so the player's zero is
// startOffset into the title; everything else here speaks title time.
function playback() {
  const state = playerManager.getPlayerState();
  return {
    currentTime: clockBase() + (playerManager.getCurrentTimeSec() || 0),
    duration: playerManager.getDurationSec(),
    active: state !== cast.framework.messages.PlayerState.IDLE,
    playing:
      state === cast.framework.messages.PlayerState.PLAYING ||
      state === cast.framework.messages.PlayerState.BUFFERING
  };
}

function clockBase() {
  if (ttsState) return ttsState.baseMs / 1000;
  return audiobookState ? 0 : sessionState.startOffset;
}

function sendStatus() {
  const media = playback();
  sendToBroadcast({
    type: 'STATUS_RESPONSE',
    payload: {
      currentTime: media.currentTime,
      duration: sessionState.duration,
      playing: media.playing,
      mediaId: sessionState.mediaId,
      mediaType: sessionState.mediaType,
      metadata: sessionState.metadata,
      isAudiobook: !!audiobookState,
      isTts: !!ttsState
    }
  });
}

// ── What this device decodes ───────────────────────────────────

const PROBE_HEIGHTS = [2160, 1440, 1080, 720];
let probedCapabilities = null;

function canPlay(mime, codecs, height) {
  try {
    return height
      ? context.canDisplayType(mime, codecs, Math.round((height * 16) / 9), height, 30)
      : context.canDisplayType(mime, codecs);
  } catch {
    return false;
  }
}

// A device's name says nothing about its generation, so senders ask the device itself.
// One height for every codec: the transcode it caps is H.264.
function receiverCapabilities() {
  if (probedCapabilities) return probedCapabilities;
  const maxHeight =
    PROBE_HEIGHTS.find((h) => canPlay('video/mp4', h > 1080 ? 'avc1.640033' : 'avc1.640028', h)) ||
    720;
  const video_codecs = ['h264'];
  if (canPlay('video/mp4', `hvc1.1.6.${maxHeight > 1080 ? 'L150' : 'L120'}.90`, maxHeight)) {
    video_codecs.push('hevc');
  }
  if (canPlay('video/mp4', 'vp09.00.40.08', maxHeight)) video_codecs.push('vp9');
  const audio_codecs = ['aac'];
  if (canPlay('audio/mp4', 'ac-3')) audio_codecs.push('ac3');
  if (canPlay('audio/mp4', 'ec-3')) audio_codecs.push('eac3');
  const probed = { video_codecs, audio_codecs, containers: ['mp4'], max_height: maxHeight };
  const hdr_formats = displayedHdr();
  if (!hdr_formats) return probed;
  probedCapabilities = { ...probed, hdr_formats };
  return probedCapabilities;
}

// Null until the receiver has started, and from firmware too old to say.
function displayedHdr() {
  let device;
  try {
    device = context.getDeviceCapabilities();
  } catch {
    return null;
  }
  if (!device) return null;
  const formats = [];
  if (device.is_hdr_supported) formats.push('hdr10', 'hlg');
  if (device.is_dv_supported) formats.push('dolby_vision');
  return formats;
}

function sendCapabilities() {
  sendToBroadcast({ type: 'CAPABILITIES', payload: receiverCapabilities() });
}

// ── Tracks ─────────────────────────────────────────────────────

let activeSubtitleId = null;

// Only WebVTT reaches the player as text: ASS needs a renderer, bitmaps a burn-in.
function textTracks() {
  return (sessionState.subtitles || [])
    .filter((s) => s.url && s.format === 'vtt')
    .map((sub, i) => ({ id: i + 1, sub }));
}

function subtitleUrl(url) {
  const qualified = qualify(url);
  if (!(sessionState.startOffset > 0)) return appendToken(qualified);
  const sep = qualified.includes('?') ? '&' : '?';
  return appendToken(`${qualified}${sep}offset=${sessionState.startOffset}`);
}

function defaultTracks() {
  activeSubtitleId = textTracks().find((t) => t.sub.is_default)?.id ?? null;
  sessionState.activeAudioIndex =
    (sessionState.audioTracks || []).find((a) => a.is_default)?.stream_index ??
    sessionState.audioTracks?.[0]?.stream_index ??
    null;
}

function sendTracks() {
  if (audiobookState || !sessionState.sessionId) return;
  sendToBroadcast({
    type: 'TRACKS',
    payload: {
      subtitles: textTracks().map(({ id, sub }) => ({
        id,
        language: sub.language ?? null,
        title: sub.title ?? null,
        forced: !!sub.is_forced,
        hearingImpaired: !!sub.is_hearing_impaired
      })),
      activeSubtitleId,
      audioTracks: (sessionState.audioTracks || []).map((a) => ({
        streamIndex: a.stream_index,
        language: a.language ?? null,
        title: a.title ?? null,
        codec: a.codec ?? null,
        channels: a.channels ?? null
      })),
      activeAudioIndex: sessionState.activeAudioIndex ?? null
    }
  });
}

// ── Session Initialization ─────────────────────────────────────

let enrichmentSource = null;

function initSession(payload) {
  sessionState = { ...payload, startOffset: payload.startOffset || 0, _isDebrid: false };
  seekGeneration = 0;
  thumbnailEntries = [];
  tokenCreatedAt = Date.now();
  currentChapterIndex = -1;
  announcedSkip = undefined;
  defaultTracks();
  flashHud();

  // Detect debrid session (session IDs from debrid start enrichment polling)
  // If no thumbnails/skip segments yet, this is likely a debrid session that needs enrichment
  const needsEnrichment =
    !sessionState.thumbnailsUrl && !sessionState.skipSegments?.length && sessionState.sessionId;

  // Fetch thumbnails if available
  if (sessionState.thumbnailsUrl) {
    fetchThumbnails(appendToken(sessionState.thumbnailsUrl));
  }

  // Start heartbeat
  clearInterval(heartbeatInterval);
  heartbeatInterval = setInterval(doHeartbeat, 30000);

  // Start skip segment monitoring
  clearInterval(skipCheckInterval);
  if (sessionState.skipSegments.length > 0) {
    skipCheckInterval = setInterval(checkSkipSegments, 500);
  }

  // Start chapter monitoring
  clearInterval(chapterCheckInterval);
  if (sessionState.chapters.length > 0) {
    chapterCheckInterval = setInterval(checkChapter, 1000);
  }

  // Self-poll enrichment for debrid streams (thumbnails, skip segments, subtitles)
  if (enrichmentSource) {
    enrichmentSource.close();
    enrichmentSource = null;
  }
  if (needsEnrichment) {
    startEnrichmentPolling();
  }

  // Broadcast position to sender every 5 seconds for progress saves
  clearInterval(positionInterval);
  positionInterval = setInterval(broadcastPosition, 5000);
}

function handleEnrichment(payload) {
  if (payload.duration && payload.duration > 0) {
    sessionState.duration = payload.duration;
  }
  if (payload.subtitles) {
    sessionState.subtitles = payload.subtitles;
  }
  if (payload.audioTracks) {
    sessionState.audioTracks = payload.audioTracks;
  }
  if (payload.skipSegments) {
    sessionState.skipSegments = payload.skipSegments;
    // Start skip monitoring if not already running
    if (!skipCheckInterval && sessionState.skipSegments.length > 0) {
      skipCheckInterval = setInterval(checkSkipSegments, 500);
    }
  }
  if (payload.thumbnailsUrl) {
    sessionState.thumbnailsUrl = payload.thumbnailsUrl;
    fetchThumbnails(appendToken(payload.thumbnailsUrl));
  }
}

function startEnrichmentPolling() {
  const url = appendToken(
    `${sessionState.baseUrl}/api/v1/admin/stream/${sessionState.sessionId}/status`
  );
  const es = new EventSource(url);
  enrichmentSource = es;

  es.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      switch (data.type) {
        case 'probe_complete':
          if (data.duration && data.duration > 0) {
            sessionState.duration = data.duration;
          }
          if (data.subtitles) sessionState.subtitles = data.subtitles;
          if (data.audio_tracks) sessionState.audioTracks = data.audio_tracks;
          sessionState._isDebrid = true;
          break;
        case 'skip_segments_ready':
          if (data.segments) {
            sessionState.skipSegments = data.segments;
            if (!skipCheckInterval) {
              skipCheckInterval = setInterval(checkSkipSegments, 500);
            }
          }
          break;
        case 'thumbnails_ready':
          if (data.thumbnails_url) {
            const thumbUrl = data.thumbnails_url.startsWith('http')
              ? data.thumbnails_url
              : `${sessionState.baseUrl}${data.thumbnails_url}`;
            sessionState.thumbnailsUrl = thumbUrl;
            fetchThumbnails(appendToken(thumbUrl));
          }
          // All enrichment done — close SSE
          es.close();
          enrichmentSource = null;
          break;
      }
    } catch {
      // Ignore malformed SSE
    }
  };

  es.onerror = () => {
    // EventSource auto-reconnects; if persistent failure, it'll timeout
  };
}

// ── Read-Aloud Mode ────────────────────────────────────────────

const TTS_POSITION_MS = 1000;
const TTS_MAX_REOPENS = 3;

// The phone highlights the word being spoken from these, so they come every second
// rather than every five.
function initTts(payload) {
  cleanup();
  sessionState = freshSession();
  ttsState = { ...payload, baseMs: 0, reopens: 0 };
  clearInterval(positionInterval);
  positionInterval = setInterval(announcePosition, TTS_POSITION_MS);
  loadTts(payload.startMs || 0);
  flashHud();
}

// The stream has no length and cannot seek, so a read is reopened at a time instead:
// the server starts it at the frame holding fromMs, and the player's zero is there.
function loadTts(fromMs) {
  ttsState.baseMs = fromMs;
  const sep = ttsState.url.includes('?') ? '&' : '?';
  const url = fromMs > 0 ? `${ttsState.url}${sep}from_ms=${fromMs}` : ttsState.url;

  const mediaInfo = new cast.framework.messages.MediaInformation();
  mediaInfo.contentUrl = url;
  mediaInfo.contentId = url;
  mediaInfo.contentType = 'audio/aac';
  mediaInfo.streamType = cast.framework.messages.StreamType.LIVE;
  const metadata = new cast.framework.messages.MusicTrackMediaMetadata();
  metadata.title = ttsState.title;
  if (ttsState.subtitle) metadata.artist = ttsState.subtitle;
  if (ttsState.coverUrl) metadata.images = [new cast.framework.messages.Image(ttsState.coverUrl)];
  mediaInfo.metadata = metadata;
  artEl.style.backgroundImage = ttsState.coverUrl ? `url("${ttsState.coverUrl}")` : '';

  const loadRequest = new cast.framework.messages.LoadRequestData();
  loadRequest.media = mediaInfo;
  loadRequest.currentTime = 0;
  loadRequest.autoplay = true;
  playerManager.load(loadRequest).catch((err) => {
    console.error('[Kobbler] Read-aloud load failed:', err);
  });
}

// A proxy resets a stream left idle through a long pause; the server still holds the
// read, so it is picked up from where the listener was.
function reopenTts() {
  if (!ttsState || ttsState.reopens >= TTS_MAX_REOPENS) return;
  ttsState.reopens++;
  loadTts(Math.floor(playback().currentTime * 1000));
}

function stopTts() {
  if (!ttsState) return;
  cleanup();
  playerManager.stop();
}

playerManager.addEventListener(cast.framework.events.EventType.ERROR, reopenTts);
playerManager.addEventListener(cast.framework.events.EventType.PLAYING, () => {
  if (ttsState) ttsState.reopens = 0;
});

// ── Audiobook Mode ─────────────────────────────────────────────

function initAudiobook(payload) {
  // Clean up any existing video session
  cleanup();

  audiobookState = payload;
  audiobookFileIndex = payload.currentFileIndex || 0;
  // Nothing of a title played before may follow the book: a seek would go to its transcode.
  sessionState = {
    ...freshSession(),
    baseUrl: payload.baseUrl,
    token: payload.token,
    duration: payload.duration
  };
  sessionState.chapters = audiobookChapters(audiobookFileIndex);

  // Start position broadcasting
  clearInterval(positionInterval);
  positionInterval = setInterval(broadcastPosition, 5000);

  // Start chapter monitoring
  clearInterval(chapterCheckInterval);
  if ((payload.chapters || []).length > 0) {
    chapterCheckInterval = setInterval(checkChapter, 1000);
  }

  // Load the current audio file via CAF
  loadAudiobookFile(audiobookFileIndex, payload.currentTime);
  flashHud();
}

// Senders give a chapter's file and its times within that file. The web sender names
// only the file it was on, so a chapter without a file belongs to whichever is playing.
function audiobookChapters(fileIndex) {
  return (audiobookState.chapters || [])
    .filter((c) => c.fileIndex === undefined || c.fileIndex === fileIndex)
    .map((c) => ({
      title: c.title,
      start_seconds: c.startSeconds ?? c.start_seconds,
      end_seconds: c.endSeconds ?? c.end_seconds
    }));
}

function seekAudiobook(fileIndex, time) {
  if (!audiobookState) return;
  if (fileIndex === audiobookFileIndex) {
    playerManager.seek(time);
  } else {
    loadAudiobookFile(fileIndex, time);
  }
  flashHud();
}

// The book's clock across its files, where the sender says how long each one is.
function audiobookClock(fileTime) {
  const lengths = audiobookState.fileDurations || [];
  if (lengths.length !== audiobookState.fileIds.length) {
    return { currentTime: fileTime, duration: lengths[audiobookFileIndex] || 0 };
  }
  const before = lengths.slice(0, audiobookFileIndex).reduce((a, b) => a + b, 0);
  return {
    currentTime: before + fileTime,
    duration: lengths.reduce((a, b) => a + b, 0)
  };
}

function loadAudiobookFile(fileIndex, startTime) {
  if (!audiobookState || fileIndex < 0 || fileIndex >= audiobookState.fileIds.length) return;

  audiobookFileIndex = fileIndex;
  const fileId = audiobookState.fileIds[fileIndex];
  // MP3 rewrapped in MP4 by the server: a browser's MP3 demuxer guesses where a seek
  // lands, and the transcript runs on what it reports.
  const audioUrl = appendToken(
    `${audiobookState.baseUrl}/api/v1/admin/books/${audiobookState.bookId}/audio/${fileId}?container=mp4`
  );

  // Build CAF media info
  const mediaInfo = new cast.framework.messages.MediaInformation();
  mediaInfo.contentUrl = audioUrl;
  mediaInfo.contentId = audioUrl;
  mediaInfo.contentType = 'audio/mp4';
  mediaInfo.streamType = cast.framework.messages.StreamType.BUFFERED;
  const fileDuration = audiobookState.fileDurations?.[fileIndex];
  if (fileDuration > 0) mediaInfo.duration = fileDuration;
  sessionState.duration = fileDuration || 0;

  // Set metadata
  const metadata = new cast.framework.messages.MusicTrackMediaMetadata();
  metadata.title = audiobookState.metadata.title;
  if (audiobookState.metadata.author) {
    metadata.artist = audiobookState.metadata.author;
  }
  metadata.trackNumber = fileIndex + 1;
  metadata.totalTracks = audiobookState.fileIds.length;
  artEl.style.backgroundImage = audiobookState.metadata.coverUrl
    ? `url("${appendToken(audiobookState.metadata.coverUrl)}")`
    : '';
  if (audiobookState.metadata.coverUrl) {
    metadata.images = [
      new cast.framework.messages.Image(appendToken(audiobookState.metadata.coverUrl))
    ];
  }
  mediaInfo.metadata = metadata;

  // Load via player manager
  const loadRequest = new cast.framework.messages.LoadRequestData();
  loadRequest.media = mediaInfo;
  loadRequest.currentTime = startTime || 0;
  loadRequest.autoplay = true;
  if (audiobookState.playbackRate) loadRequest.playbackRate = audiobookState.playbackRate;

  playerManager.load(loadRequest).then(
    () => console.log('[Kobbler] Audiobook file loaded:', fileIndex),
    (err) => console.error('[Kobbler] Audiobook load failed:', err)
  );

  sessionState.chapters = audiobookChapters(fileIndex);
  currentChapterIndex = -1;
}

function audiobookAdvanceFile() {
  if (!audiobookState) return;
  const nextIndex = audiobookFileIndex + 1;
  if (nextIndex < audiobookState.fileIds.length) {
    loadAudiobookFile(nextIndex, 0);
    sendToBroadcast({
      type: 'AUDIOBOOK_FILE_CHANGED',
      payload: { fileIndex: nextIndex, fileId: audiobookState.fileIds[nextIndex] }
    });
  } else {
    // Book finished
    sendToBroadcast({ type: 'MEDIA_FINISHED' });
  }
}

function saveAudiobookPosition(pos) {
  if (pos <= 0 || pos === lastSavedPosition || !audiobookState) return;
  lastSavedPosition = pos;

  const fileId = audiobookState.fileIds[audiobookFileIndex];
  const book = audiobookClock(pos);
  const progress = book.duration > 0 ? Math.round((book.currentTime / book.duration) * 100) : 0;

  fetch(
    appendToken(
      `${audiobookState.baseUrl}/api/v1/admin/books/${audiobookState.bookId}/audio-position`
    ),
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        file_id: fileId,
        position_seconds: pos,
        progress_percent: progress
      })
    }
  ).catch(() => {});
}

let lastSavedPosition = -1;

function announcePosition() {
  const media = playback();
  if (!media.active) return null;
  const update = {
    currentTime: media.currentTime,
    duration: knownDuration(media),
    playing: media.playing
  };
  sendToBroadcast({ type: 'POSITION_UPDATE', payload: update });
  return update;
}

function broadcastPosition() {
  const update = announcePosition();
  if (!update || !update.playing) return;

  // Save position directly to Kobbler API (survives sender disconnect)
  if (audiobookState) {
    saveAudiobookPosition(Math.floor(update.currentTime));
  } else {
    savePositionToApi(Math.floor(update.currentTime), update.duration);
  }
}

playerManager.addEventListener(cast.framework.events.EventType.PAUSE, announcePosition);
playerManager.addEventListener(cast.framework.events.EventType.PLAYING, announcePosition);

// A playlist the server is still transcoding grows as it goes, so the element only
// knows the part written so far.
function knownDuration(media) {
  if (sessionState.duration > 0) return sessionState.duration;
  return Number.isFinite(media.duration) ? media.duration : 0;
}

function savePositionToApi(pos, dur) {
  if (pos <= 0 || pos === lastSavedPosition || !sessionState.mediaId) return;
  lastSavedPosition = pos;

  let url;
  if (sessionState.mediaType === 'movie') {
    url = `${sessionState.baseUrl}/api/v1/admin/movies/${sessionState.mediaId}/watch-state/position`;
  } else if (sessionState.mediaType === 'episode') {
    url = `${sessionState.baseUrl}/api/v1/admin/tv/episodes/${sessionState.mediaId}/watch-state/position`;
  } else {
    return; // live/recording — no position save
  }

  fetch(appendToken(url), {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ position_seconds: pos })
  }).catch(() => {});

  // Auto-mark watched at 90%
  if (dur > 0 && pos / dur >= 0.9 && !sessionState._markedWatched) {
    sessionState._markedWatched = true;
    const watchUrl =
      sessionState.mediaType === 'movie'
        ? `${sessionState.baseUrl}/api/v1/admin/movies/${sessionState.mediaId}/watch-state`
        : `${sessionState.baseUrl}/api/v1/admin/tv/episodes/${sessionState.mediaId}/watch-state`;
    fetch(appendToken(watchUrl), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ watched: true, watched_at: new Date().toISOString() })
    }).catch(() => {});
  }
}

// ── Auto-Play Next Episode ─────────────────────────────────────

async function playNextEpisode() {
  const episodeId = sessionState.mediaId;
  if (!episodeId) return;

  // Mark current as watched
  savePositionToApi(Math.floor(sessionState.duration), sessionState.duration);

  try {
    // 1. Get next episode info
    const nextResp = await fetch(
      appendToken(`${sessionState.baseUrl}/api/v1/admin/tv/episodes/${episodeId}/next`)
    );
    if (!nextResp.ok) {
      // No next episode — series finished
      cleanup();
      sendToBroadcast({ type: 'MEDIA_FINISHED' });
      return;
    }
    const nextEpisode = await nextResp.json();

    // Destroy current session
    if (sessionState.sessionId) {
      fetch(
        appendToken(
          `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}`
        ),
        { method: 'DELETE' }
      ).catch(() => {});
    }

    // 2. Try debrid first if current session was debrid
    let newSession = null;
    if (sessionState._isDebrid) {
      newSession = await resolveDebridNextEpisode(nextEpisode.episode_id);
    }

    // 3. Fall back to local file
    if (!newSession && nextEpisode.file_id) {
      newSession = await createLocalSession(nextEpisode.file_id);
    }

    if (!newSession) {
      cleanup();
      sendToBroadcast({ type: 'MEDIA_FINISHED' });
      return;
    }

    // 4. Update session state and load new media
    sessionState.sessionId = newSession.session_id;
    sessionState.mediaId = nextEpisode.episode_id;
    sessionState.playlistUrl = newSession.playlist_url;
    sessionState.directUrl = newSession.direct_url;
    sessionState.method = newSession.playlist_url ? 'hls' : 'direct_play';
    sessionState.duration = newSession.duration_seconds;
    sessionState.startOffset = 0;
    sessionState.subtitles = newSession.subtitles || [];
    sessionState.audioTracks = newSession.audio_tracks || [];
    sessionState.thumbnailsUrl = newSession.thumbnails_url || null;
    sessionState.chapters = newSession.chapters || [];
    sessionState.skipSegments = newSession.skip_segments || [];
    sessionState._isDebrid = !!newSession._isDebrid;
    defaultTracks();
    announcedSkip = undefined;

    // Reset enrichment state
    seekGeneration = 0;
    thumbnailEntries = [];
    currentChapterIndex = -1;
    lastSavedPosition = -1;

    // Re-fetch thumbnails
    if (sessionState.thumbnailsUrl) {
      fetchThumbnails(appendToken(qualify(sessionState.thumbnailsUrl)));
    }

    // Start skip monitoring
    clearInterval(skipCheckInterval);
    if (sessionState.skipSegments.length > 0) {
      skipCheckInterval = setInterval(checkSkipSegments, 500);
    }

    // Start enrichment polling for debrid
    if (enrichmentSource) {
      enrichmentSource.close();
      enrichmentSource = null;
    }
    if (sessionState._isDebrid) {
      startEnrichmentPolling();
    }

    // Load the new media on the player
    const contentUrl = qualify(sessionState.playlistUrl || sessionState.directUrl);
    const contentType =
      sessionState.method === 'direct_play' ? 'video/mp4' : 'application/x-mpegURL';

    const mediaInfo = new cast.framework.messages.MediaInformation();
    mediaInfo.contentUrl = appendToken(contentUrl);
    mediaInfo.contentId = appendToken(contentUrl);
    mediaInfo.contentType = contentType;
    if (sessionState.method === 'hls') {
      mediaInfo.hlsSegmentFormat = cast.framework.messages.HlsSegmentFormat.FMP4;
    }

    const metadata = new cast.framework.messages.GenericMediaMetadata();
    metadata.title = nextEpisode.show_title || 'Next Episode';
    metadata.subtitle = `S${nextEpisode.season_identifier}E${nextEpisode.episode_identifier}${nextEpisode.title ? ' - ' + nextEpisode.title : ''}`;
    mediaInfo.metadata = metadata;
    sessionState.metadata = {
      ...sessionState.metadata,
      title: metadata.title,
      subtitle: metadata.subtitle
    };

    const loadRequest = new cast.framework.messages.LoadRequestData();
    loadRequest.media = mediaInfo;
    loadRequest.autoplay = true;

    playerManager.load(loadRequest).then(
      () => {
        console.log('[Kobbler] Next episode loaded:', nextEpisode.episode_id);
        sendToBroadcast({
          type: 'EPISODE_CHANGED',
          payload: {
            episodeId: nextEpisode.episode_id,
            title: nextEpisode.title,
            seasonIdentifier: nextEpisode.season_identifier,
            episodeIdentifier: nextEpisode.episode_identifier,
            showTitle: nextEpisode.show_title
          }
        });
      },
      (err) => {
        console.error('[Kobbler] Failed to load next episode:', err);
        cleanup();
        sendToBroadcast({ type: 'MEDIA_FINISHED' });
      }
    );
  } catch (err) {
    console.error('[Kobbler] Next episode error:', err);
    cleanup();
    sendToBroadcast({ type: 'MEDIA_FINISHED' });
  }
}

async function createLocalSession(fileId) {
  const resp = await fetch(appendToken(`${sessionState.baseUrl}/api/v1/admin/playback/sessions`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      media_type: 'episode',
      file_id: fileId,
      capabilities: receiverCapabilities()
    })
  });
  if (!resp.ok) return null;
  return resp.json();
}

async function resolveDebridNextEpisode(episodeId) {
  // Search for streams
  const searchResp = await fetch(
    appendToken(`${sessionState.baseUrl}/api/v1/admin/stream/search`),
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ episode_id: episodeId })
    }
  );
  if (!searchResp.ok) return null;

  const searchData = await searchResp.json();
  const options = searchData.options || searchData.results || [];
  if (options.length === 0) return null;

  // Try resolving the top options
  for (const option of options.slice(0, 5)) {
    try {
      const resolveResp = await fetch(
        appendToken(`${sessionState.baseUrl}/api/v1/admin/stream/resolve`),
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            debrid_provider_id: option.debrid_provider_id,
            info_hash: option.info_hash,
            file_id: option.file_id,
            media_type: 'episode',
            media_id: episodeId,
            release_title: option.release_title,
            debrid_provider: option.debrid_provider,
            quality: option.quality,
            capabilities: receiverCapabilities()
          })
        }
      );
      if (!resolveResp.ok) continue;

      const resolved = await resolveResp.json();
      // Skip if still caching (receiver can't easily wait for progressive download)
      if (resolved.status === 'caching') continue;

      if (resolved.session_id) {
        resolved._isDebrid = true;
        return resolved;
      }
    } catch {
      continue;
    }
  }
  return null;
}

function qualify(url) {
  if (!url) return url;
  if (url.startsWith('http')) return url;
  return sessionState.baseUrl + url;
}

// ── LOAD Interceptor ───────────────────────────────────────────

playerManager.setMessageInterceptor(cast.framework.messages.MessageType.LOAD, (loadRequestData) => {
  // If sender sends session info via customData, use it
  if (loadRequestData.media && loadRequestData.media.customData) {
    const custom = loadRequestData.media.customData;
    if (custom.kobblerSession) {
      initSession(custom.kobblerSession);
    }
  }

  // Ensure the media URL has our auth token
  if (loadRequestData.media && loadRequestData.media.contentUrl) {
    loadRequestData.media.contentUrl = appendToken(loadRequestData.media.contentUrl);
  }
  if (loadRequestData.media && loadRequestData.media.contentId) {
    loadRequestData.media.contentId = appendToken(loadRequestData.media.contentId);
  }
  if (audiobookState || ttsState) return loadRequestData;

  // The playlist is EVENT-typed until the transcode finishes, which CAF reads as live.
  loadRequestData.media.streamType = cast.framework.messages.StreamType.BUFFERED;
  if (sessionState.duration > 0) {
    loadRequestData.media.duration = sessionState.duration;
  }

  // Set content type based on playback method
  if (sessionState.method === 'hls' && sessionState.playlistUrl) {
    loadRequestData.media.contentType = 'application/x-mpegURL';
    loadRequestData.media.hlsSegmentFormat = cast.framework.messages.HlsSegmentFormat.FMP4;
  } else if (sessionState.method === 'direct_play') {
    loadRequestData.media.contentType = 'video/mp4';
  }

  // Set metadata for display on TV
  if (sessionState.metadata) {
    const meta = sessionState.metadata;
    loadRequestData.media.metadata =
      loadRequestData.media.metadata || new cast.framework.messages.GenericMediaMetadata();
    if (meta.title) loadRequestData.media.metadata.title = meta.title;
    if (meta.subtitle) loadRequestData.media.metadata.subtitle = meta.subtitle;
    if (meta.posterUrl) {
      loadRequestData.media.metadata.images = [
        new cast.framework.messages.Image(appendToken(meta.posterUrl))
      ];
    }
  }

  const texts = textTracks();
  if (texts.length > 0) {
    loadRequestData.media.tracks = texts.map(({ id, sub }) => {
      const track = new cast.framework.messages.Track(id, cast.framework.messages.TrackType.TEXT);
      track.trackContentId = subtitleUrl(sub.url);
      track.trackContentType = 'text/vtt';
      track.subtype = cast.framework.messages.TextTrackType.SUBTITLES;
      track.name = sub.title || sub.language || `Track ${id}`;
      track.language = sub.language || 'und';
      return track;
    });
    loadRequestData.activeTrackIds = activeSubtitleId ? [activeSubtitleId] : [];
  }

  return loadRequestData;
});

playerManager.setMessageInterceptor(cast.framework.messages.MessageType.MEDIA_STATUS, (status) => {
  if (ttsState && typeof status.currentTime === 'number') status.currentTime += clockBase();
  if (audiobookState || !sessionState.sessionId) return status;
  if (typeof status.currentTime === 'number') status.currentTime += sessionState.startOffset;
  delete status.liveSeekableRange;
  if (status.media) {
    status.media.streamType = cast.framework.messages.StreamType.BUFFERED;
    if (sessionState.duration > 0) status.media.duration = sessionState.duration;
  }
  return status;
});

// ── SEEK Interceptor (Generation-Based) ────────────────────────

playerManager.setMessageInterceptor(cast.framework.messages.MessageType.SEEK, async (seekData) => {
  if (!sessionState.sessionId) {
    return seekData;
  }

  const targetTime = seekData.currentTime;
  showThumbnailAtTime(targetTime);
  flashHud();

  // Direct play: native seek via range requests, no server interaction needed
  if (sessionState.method === 'direct_play') {
    setTimeout(hideThumbnail, 500);
    return seekData;
  }

  const handled = await seekTranscode(targetTime);
  setTimeout(hideThumbnail, 500);
  // null suppresses CAF's own seek: the server rebuilt the playlist from the target.
  return handled ? null : seekData;
});

async function seekTranscode(targetTime) {
  // A seek must not resume a receiver the viewer paused. The reload below is
  // our own machinery, not a request to play.
  const wasPaused = playerManager.getPlayerState() === cast.framework.messages.PlayerState.PAUSED;
  try {
    const resp = await fetch(
      appendToken(
        `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}/seek`
      ),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ time_seconds: targetTime })
      }
    );
    if (!resp.ok) return false;

    const data = await resp.json();
    seekGeneration = data.seek_generation;
    sessionState.startOffset = data.start_time;

    reloadAt(
      appendToken(appendGeneration(sessionState.playlistUrl, seekGeneration)),
      data.sub_segment_offset || 0,
      !wasPaused
    );
    return true;
  } catch (err) {
    console.error('[Kobbler] Seek failed:', err);
    return false;
  }
}

function reloadAt(url, currentTime, autoplay) {
  const mediaInfo = playerManager.getMediaInformation();
  if (!mediaInfo) return;
  mediaInfo.contentUrl = url;
  mediaInfo.contentId = url;
  // Left on, the LOAD interceptor would re-initialise from the original handover.
  delete mediaInfo.customData;

  const loadRequest = new cast.framework.messages.LoadRequestData();
  loadRequest.media = mediaInfo;
  loadRequest.currentTime = currentTime;
  loadRequest.autoplay = autoplay;
  playerManager.load(loadRequest).catch((err) => {
    console.error('[Kobbler] Reload failed:', err);
  });
}

function seekTitle(target) {
  if (sessionState.sessionId && sessionState.method !== 'direct_play' && !audiobookState) {
    return seekTranscode(target);
  }
  playerManager.seek(target);
}

// ── Media Status Events ────────────────────────────────────────

playerManager.addEventListener(cast.framework.events.EventType.MEDIA_FINISHED, (event) => {
  // CAF also "finishes" a title it replaces with a new LOAD; only a real end may
  // mark it watched or move on.
  if (event.endedReason !== cast.framework.events.EndedReason.END_OF_STREAM) return;

  // The read's chapter, not the session: the sender decides what is read next.
  if (ttsState) {
    sendToBroadcast({ type: 'TTS_FINISHED' });
    return;
  }

  // Audiobook: advance to next file
  if (audiobookState) {
    audiobookAdvanceFile();
    return;
  }

  // Episode: auto-play next episode
  if (sessionState.mediaType === 'episode' && sessionState.mediaId) {
    playNextEpisode();
    return;
  }

  // Movie or unknown: done
  cleanup();
  sendToBroadcast({ type: 'MEDIA_FINISHED' });
});

playerManager.addEventListener(cast.framework.events.EventType.PLAYER_LOAD_COMPLETE, () => {
  hideThumbnail();
  hideSkipOverlay();
  flashHud();
  sendTracks();
});

// ── Skip Segments ──────────────────────────────────────────────

function checkSkipSegments() {
  const media = playback();
  if (!media.active) return;

  const t = media.currentTime;
  const active = sessionState.skipSegments.find((s) => t >= s.start_seconds && t < s.end_seconds);

  if (active) {
    showSkipOverlay(active);
  } else {
    hideSkipOverlay();
  }

  const skip = active ? { segmentType: active.segment_type, endSeconds: active.end_seconds } : null;
  if (JSON.stringify(skip) !== JSON.stringify(announcedSkip)) {
    announcedSkip = skip;
    sendToBroadcast({ type: 'SKIP_SEGMENT', payload: skip });
  }
}

// undefined until announced, so a sender that joins is told even when there is nothing.
let announcedSkip = undefined;

function showSkipOverlay(segment) {
  const label =
    segment.segment_type === 'intro'
      ? 'Skip Intro'
      : segment.segment_type === 'credits'
        ? 'Skip Credits'
        : segment.segment_type === 'recap'
          ? 'Skip Recap'
          : segment.segment_type === 'post_credits_scene'
            ? 'Skip to Post-Credits'
            : 'Skip';

  skipBtn.textContent = label;
  skipBtn.dataset.skipTo = segment.end_seconds;
  skipOverlay.classList.add('visible');
  skipOverlay.setAttribute('aria-hidden', 'false');
  // Auto-focus for Google TV remote D-pad navigation
  skipBtn.focus();
}

function hideSkipOverlay() {
  skipOverlay.classList.remove('visible');
  skipOverlay.setAttribute('aria-hidden', 'true');
}

// Skip button click/select handler
skipBtn.addEventListener('click', () => {
  const skipTo = parseFloat(skipBtn.dataset.skipTo);
  if (isNaN(skipTo)) return;
  seekTitle(skipTo);
});

// D-pad enter key support for Google TV
skipBtn.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    skipBtn.click();
  }
});

// ── Thumbnail Preview ──────────────────────────────────────────

async function fetchThumbnails(url) {
  try {
    const resp = await fetch(url);
    if (!resp.ok) return;
    const vtt = await resp.text();
    thumbnailEntries = parseVttThumbnails(vtt, url);
  } catch (err) {
    console.error('[Kobbler] Failed to fetch thumbnails:', err);
  }
}

function showThumbnailAtTime(time) {
  const entry = findThumbnailAtTime(thumbnailEntries, time);
  if (!entry) {
    hideThumbnail();
    return;
  }

  thumbnailPreview.style.width = entry.w + 'px';
  thumbnailPreview.style.height = entry.h + 'px';
  thumbnailPreview.style.backgroundImage = `url(${entry.url})`;
  thumbnailPreview.style.backgroundPosition = `-${entry.x}px -${entry.y}px`;
  thumbnailPreview.classList.add('visible');

  // Show time label
  let label = thumbnailPreview.querySelector('.time-label');
  if (!label) {
    label = document.createElement('div');
    label.className = 'time-label';
    thumbnailPreview.appendChild(label);
  }
  label.textContent = formatTime(time);
}

function hideThumbnail() {
  thumbnailPreview.classList.remove('visible');
}

// ── Chapter Display ────────────────────────────────────────────

function checkChapter() {
  const media = playback();
  if (!media.active) return;

  const t = media.currentTime;
  const idx = sessionState.chapters.findIndex((c) => t >= c.start_seconds && t < c.end_seconds);

  if (idx !== currentChapterIndex) {
    currentChapterIndex = idx;
    if (idx >= 0 && sessionState.chapters[idx].title) {
      showChapter(sessionState.chapters[idx].title);
    }
  }
}

let chapterHideTimeout = null;

function showChapter(title) {
  let titleEl = chapterDisplay.querySelector('.chapter-title');
  if (!titleEl) {
    titleEl = document.createElement('div');
    titleEl.className = 'chapter-title';
    chapterDisplay.appendChild(titleEl);
  }
  titleEl.textContent = title;
  chapterDisplay.classList.add('visible');

  clearTimeout(chapterHideTimeout);
  chapterHideTimeout = setTimeout(() => {
    chapterDisplay.classList.remove('visible');
  }, 5000);
}

// ── Subtitle / Audio Handling ──────────────────────────────────

function handleSubtitleChange(payload) {
  activeSubtitleId = payload.trackId ?? null;
  try {
    playerManager.getTextTracksManager().setActiveByIds(activeSubtitleId ? [activeSubtitleId] : []);
  } catch (err) {
    console.error('[Kobbler] Subtitle change failed:', err);
  }
  sendTracks();
}

async function handleAudioSwitch(payload) {
  if (!sessionState.sessionId) return;
  const resumeTime = playback().currentTime;
  const wasPaused = playerManager.getPlayerState() === cast.framework.messages.PlayerState.PAUSED;
  try {
    const resp = await fetch(
      appendToken(
        `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}/switch-audio`
      ),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ audio_stream_index: payload.streamIndex, resume_time: resumeTime })
      }
    );
    if (!resp.ok) return;
    const data = await resp.json();

    sessionState.activeAudioIndex = payload.streamIndex;
    if (data.audio_tracks?.length) sessionState.audioTracks = data.audio_tracks;
    const hls = data.method !== 'direct_play';
    sessionState.method = hls ? 'hls' : 'direct_play';
    if (hls) {
      sessionState.playlistUrl = qualify(data.playlist_url);
      seekGeneration += 1;
    } else {
      sessionState.directUrl = qualify(data.direct_url);
    }
    // The new transcode starts from the resume point and calls it zero.
    sessionState.startOffset = hls ? data.start_time || 0 : 0;
    reloadAt(
      appendToken(
        hls ? appendGeneration(sessionState.playlistUrl, seekGeneration) : sessionState.directUrl
      ),
      Math.max(0, resumeTime - sessionState.startOffset),
      !wasPaused
    );
    sendTracks();
  } catch (err) {
    console.error('[Kobbler] Audio switch failed:', err);
  }
}

// ── Heartbeat ──────────────────────────────────────────────────

async function doHeartbeat() {
  if (!sessionState.sessionId) return;

  try {
    const resp = await fetch(
      appendToken(
        `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}/heartbeat`
      )
    );

    if (!resp.ok) {
      console.warn('[Kobbler] Heartbeat failed, session may have expired');
      sendToBroadcast({ type: 'SESSION_EXPIRED' });
    }
  } catch (err) {
    console.error('[Kobbler] Heartbeat error:', err);
  }

  // Self-refresh token before expiry (receiver is independent of sender)
  if (Date.now() - tokenCreatedAt > TOKEN_REFRESH_AFTER_MS) {
    refreshToken();
  }
}

async function refreshToken() {
  try {
    const resp = await fetch(appendToken(`${sessionState.baseUrl}/api/v1/admin/playback/tokens`), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ttl_hours: 24 })
    });
    if (resp.ok) {
      const data = await resp.json();
      const oldToken = sessionState.token;
      sessionState.token = data.token;
      tokenCreatedAt = Date.now();
      console.log('[Kobbler] Token refreshed');

      // Notify sender of new token
      sendToBroadcast({ type: 'TOKEN_REFRESH', payload: { token: data.token } });

      // Revoke old token (best-effort)
      fetch(
        `${sessionState.baseUrl}/api/v1/admin/playback/tokens/${oldToken}?token=${data.token}`,
        {
          method: 'DELETE'
        }
      ).catch(() => {});
    }
  } catch (err) {
    console.error('[Kobbler] Token refresh failed:', err);
  }
}

// ── Cleanup ────────────────────────────────────────────────────

function cleanup() {
  clearInterval(heartbeatInterval);
  clearInterval(skipCheckInterval);
  clearInterval(chapterCheckInterval);
  clearInterval(positionInterval);
  clearTimeout(chapterHideTimeout);
  hideSkipOverlay();
  hideThumbnail();
  audiobookState = null;
  ttsState = null;
  if (enrichmentSource) {
    enrichmentSource.close();
    enrichmentSource = null;
  }

  if (sessionState.sessionId) {
    // Best-effort session destruction
    fetch(
      appendToken(
        `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}`
      ),
      { method: 'DELETE' }
    ).catch(() => {});
  }
}

context.addEventListener(cast.framework.system.EventType.SENDER_CONNECTED, () => {
  sendCapabilities();
  sendTracks();
  announcedSkip = undefined;
});

context.addEventListener(cast.framework.system.EventType.SENDER_DISCONNECTED, () => {
  if (context.getSenders().length === 0) {
    cleanup();
    context.stop();
  }
});

// ── VTT Thumbnail Parsing (mirrors web/src/lib/utils/vttThumbnails.ts) ──

function parseVttThumbnails(vttContent, baseUrl) {
  const entries = [];
  const lines = vttContent.split('\n');
  let i = 0;

  while (i < lines.length && !lines[i].includes('-->')) i++;

  while (i < lines.length) {
    const line = lines[i].trim();
    if (line.includes('-->')) {
      const parts = line.split('-->').map((s) => s.trim());
      const start = parseVttTime(parts[0]);
      const end = parseVttTime(parts[1]);
      i++;
      if (i < lines.length) {
        const urlLine = lines[i].trim();
        if (urlLine) {
          const parsed = parseSpriteRef(urlLine, baseUrl);
          if (parsed) {
            entries.push({ start, end, ...parsed });
          }
        }
      }
    }
    i++;
  }
  return entries;
}

function findThumbnailAtTime(entries, time) {
  if (entries.length === 0) return null;
  let lo = 0;
  let hi = entries.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const entry = entries[mid];
    if (time < entry.start) hi = mid - 1;
    else if (time >= entry.end) lo = mid + 1;
    else return entry;
  }
  return null;
}

function parseVttTime(str) {
  const parts = str.split(':');
  if (parts.length === 3) {
    return parseInt(parts[0]) * 3600 + parseInt(parts[1]) * 60 + parseFloat(parts[2]);
  } else if (parts.length === 2) {
    return parseInt(parts[0]) * 60 + parseFloat(parts[1]);
  }
  return parseFloat(str) || 0;
}

function parseSpriteRef(ref, baseUrl) {
  const hashIdx = ref.indexOf('#');
  if (hashIdx === -1) return null;
  const filename = ref.substring(0, hashIdx);
  const fragment = ref.substring(hashIdx + 1);
  if (!fragment.startsWith('xywh=')) return null;
  const coords = fragment.replace('xywh=', '').split(',').map(Number);
  const base = baseUrl.replace(/\.vtt$/, '/');
  return {
    url: appendToken(base + filename),
    x: coords[0],
    y: coords[1],
    w: coords[2],
    h: coords[3]
  };
}

// ── On-screen transport ────────────────────────────────────────

// An Android TV receiver draws its own transport from the media status, which already
// speaks title time, and takes the remote itself; a second one would sit on top of it.
const SYSTEM_TRANSPORT = /DeviceType\/AndroidTV/.test(navigator.userAgent);
let hudUntil = 0;

function flashHud(ms = 5000) {
  hudUntil = Date.now() + ms;
  renderHud();
}

function hudText() {
  if (ttsState) return { title: ttsState.title || '', subtitle: ttsState.subtitle || '' };
  if (audiobookState) {
    const t = playback().currentTime;
    const chapter = sessionState.chapters.find(
      (c) => t >= c.start_seconds && t < c.end_seconds
    )?.title;
    return {
      title: audiobookState.metadata.title || '',
      subtitle: [audiobookState.metadata.author, chapter].filter(Boolean).join(' · ')
    };
  }
  return {
    title: sessionState.metadata?.title || '',
    subtitle: sessionState.metadata?.subtitle || ''
  };
}

function renderHud() {
  const media = playback();
  const state = playerManager.getPlayerState();
  const PlayerState = cast.framework.messages.PlayerState;
  idleEl.classList.toggle('visible', !media.active);
  artEl.classList.toggle('visible', !!(audiobookState || ttsState) && media.active);
  if (!media.active) {
    hudEl.classList.remove('visible');
    return;
  }

  const clock = audiobookState
    ? audiobookClock(media.currentTime)
    : { currentTime: media.currentTime, duration: knownDuration(media) };
  const duration = clock.duration;
  const text = hudText();
  hudTitle.textContent = text.title;
  hudSubtitle.textContent = text.subtitle;
  hudElapsed.textContent = formatTime(clock.currentTime);
  hudTotal.textContent = duration > 0 ? formatTime(duration) : '';
  hudFill.style.width =
    duration > 0 ? `${Math.min(100, (clock.currentTime / duration) * 100)}%` : '0%';
  hudState.textContent =
    state === PlayerState.PAUSED ? '❚❚' : state === PlayerState.BUFFERING ? '…' : '▶';
  const show =
    !SYSTEM_TRANSPORT &&
    (!!audiobookState || !!ttsState || state !== PlayerState.PLAYING || Date.now() < hudUntil);
  hudEl.classList.toggle('visible', show);
}

setInterval(renderHud, 500);
playerManager.addEventListener(cast.framework.events.EventType.PLAYING, () => flashHud());
playerManager.addEventListener(cast.framework.events.EventType.PAUSE, () => flashHud());

// The remote's D-pad, on devices that have one.
document.addEventListener('keydown', (e) => {
  if (SYSTEM_TRANSPORT || e.target === skipBtn) return;
  const media = playback();
  if (!media.active) return;
  flashHud();
  if (ttsState && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) return;
  if (e.key === 'ArrowLeft') {
    seekTitle(Math.max(0, media.currentTime - 10));
  } else if (e.key === 'ArrowRight') {
    const end = knownDuration(media);
    seekTitle(end > 0 ? Math.min(end - 1, media.currentTime + 30) : media.currentTime + 30);
  } else if (e.key === 'Enter' || e.key === ' ') {
    if (media.playing) playerManager.pause();
    else playerManager.play();
  } else {
    return;
  }
  e.preventDefault();
});

// ── Utility Functions ──────────────────────────────────────────

function appendToken(url) {
  if (!sessionState.token || !url) return url;
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}token=${sessionState.token}`;
}

function appendGeneration(url, gen) {
  if (!url) return url;
  // Remove existing ?g= or &g= parameter
  const cleaned = url.replace(/[?&]g=\d+/, '');
  const sep = cleaned.includes('?') ? '&' : '?';
  return `${cleaned}${sep}g=${gen}`;
}

function formatTime(seconds) {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  }
  return `${m}:${String(s).padStart(2, '0')}`;
}

// ── Start the Receiver ─────────────────────────────────────────

const options = new cast.framework.CastReceiverOptions();
options.disableIdleTimeout = true;
options.customNamespaces = {
  [NAMESPACE]: cast.framework.system.MessageType.JSON
};

context.start(options);
