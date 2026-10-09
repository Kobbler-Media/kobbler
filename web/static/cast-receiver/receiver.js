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

let sessionState = {
  baseUrl: '',
  token: '',
  sessionId: '',
  mediaId: null,
  mediaType: null,
  playlistUrl: null,
  directUrl: null,
  method: 'hls',
  duration: 0,
  subtitles: [],
  audioTracks: [],
  thumbnailsUrl: null,
  chapters: [],
  skipSegments: [],
  capabilities: null,
  metadata: {},
  _isDebrid: false
};

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

// Token refresh tracking
let tokenCreatedAt = Date.now();
const TOKEN_REFRESH_AFTER_MS = 20 * 60 * 60 * 1000; // Refresh after 20 hours

// ── DOM refs ───────────────────────────────────────────────────

const skipOverlay = document.getElementById('skip-overlay');
const skipBtn = document.getElementById('skip-btn');
const thumbnailPreview = document.getElementById('thumbnail-preview');
const chapterDisplay = document.getElementById('chapter-display');

// ── CAF Setup ──────────────────────────────────────────────────

const context = cast.framework.CastReceiverContext.getInstance();
const playerManager = context.getPlayerManager();

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
  }
}

function sendToBroadcast(msg) {
  context.sendCustomMessage(NAMESPACE, undefined, msg);
}

function sendStatus() {
  const mediaEl = document.querySelector('video') || document.querySelector('audio');
  sendToBroadcast({
    type: 'STATUS_RESPONSE',
    payload: {
      currentTime: mediaEl ? mediaEl.currentTime : 0,
      duration: sessionState.duration,
      playing: mediaEl ? !mediaEl.paused : false,
      volume: mediaEl ? mediaEl.volume : 1,
      mediaId: sessionState.mediaId,
      mediaType: sessionState.mediaType,
      metadata: sessionState.metadata,
      isAudiobook: !!audiobookState
    }
  });
}

// ── Session Initialization ─────────────────────────────────────

let enrichmentSource = null;

function initSession(payload) {
  sessionState = { ...payload, _isDebrid: false };
  seekGeneration = 0;
  thumbnailEntries = [];
  tokenCreatedAt = Date.now();
  currentChapterIndex = -1;

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

// ── Audiobook Mode ─────────────────────────────────────────────

function initAudiobook(payload) {
  // Clean up any existing video session
  cleanup();

  audiobookState = payload;
  audiobookFileIndex = payload.currentFileIndex || 0;
  sessionState.baseUrl = payload.baseUrl;
  sessionState.token = payload.token;
  sessionState.chapters = payload.chapters || [];
  sessionState.duration = payload.duration;

  // Start position broadcasting
  clearInterval(positionInterval);
  positionInterval = setInterval(broadcastPosition, 5000);

  // Start chapter monitoring
  clearInterval(chapterCheckInterval);
  if (sessionState.chapters.length > 0) {
    chapterCheckInterval = setInterval(checkChapter, 1000);
  }

  // Load the current audio file via CAF
  loadAudiobookFile(audiobookFileIndex, payload.currentTime);
}

function loadAudiobookFile(fileIndex, startTime) {
  if (!audiobookState || fileIndex < 0 || fileIndex >= audiobookState.fileIds.length) return;

  audiobookFileIndex = fileIndex;
  const fileId = audiobookState.fileIds[fileIndex];
  const audioUrl = appendToken(
    `${audiobookState.baseUrl}/api/v1/admin/books/${audiobookState.bookId}/audio/${fileId}`
  );

  // Build CAF media info
  const mediaInfo = new cast.framework.messages.MediaInformation();
  mediaInfo.contentUrl = audioUrl;
  mediaInfo.contentId = audioUrl;
  mediaInfo.contentType = 'audio/mp4';
  mediaInfo.streamType = cast.framework.messages.StreamType.BUFFERED;
  if (audiobookState.duration > 0) {
    mediaInfo.duration = audiobookState.duration;
  }

  // Set metadata
  const metadata = new cast.framework.messages.MusicTrackMediaMetadata();
  metadata.title = audiobookState.metadata.title;
  if (audiobookState.metadata.author) {
    metadata.artist = audiobookState.metadata.author;
  }
  metadata.trackNumber = fileIndex + 1;
  metadata.totalTracks = audiobookState.fileIds.length;
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

  playerManager.load(loadRequest).then(
    () => console.log('[Kobbler] Audiobook file loaded:', fileIndex),
    (err) => console.error('[Kobbler] Audiobook load failed:', err)
  );

  // Update chapters for this file
  if (audiobookState.chapters) {
    sessionState.chapters = audiobookState.chapters.filter(
      (c) => c.fileIndex === undefined || c.fileIndex === fileIndex
    );
  }
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

function saveAudiobookPosition(pos, dur) {
  if (pos <= 0 || pos === lastSavedPosition || !audiobookState) return;
  lastSavedPosition = pos;

  const fileId = audiobookState.fileIds[audiobookFileIndex];
  const progress = dur > 0 ? Math.round((pos / dur) * 100) : 0;

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

function broadcastPosition() {
  // Handle both video and audiobook modes
  const mediaEl = document.querySelector('video') || document.querySelector('audio');
  if (!mediaEl || mediaEl.paused) return;
  const currentTime = mediaEl.currentTime;
  const dur = mediaEl.duration || sessionState.duration;

  // Relay to sender for UI display
  sendToBroadcast({
    type: 'POSITION_UPDATE',
    payload: { currentTime, duration: dur, playing: !mediaEl.paused }
  });

  // Save position directly to Kobbler API (survives sender disconnect)
  if (audiobookState) {
    saveAudiobookPosition(Math.floor(currentTime), dur);
  } else {
    savePositionToApi(Math.floor(currentTime), dur);
  }
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
    method: 'PUT',
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
    sessionState.subtitles = newSession.subtitles || [];
    sessionState.audioTracks = newSession.audio_tracks || [];
    sessionState.thumbnailsUrl = newSession.thumbnails_url || null;
    sessionState.chapters = newSession.chapters || [];
    sessionState.skipSegments = newSession.skip_segments || [];
    sessionState._isDebrid = !!newSession._isDebrid;

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
      capabilities: sessionState.capabilities || {
        video_codecs: ['h264', 'vp9'],
        audio_codecs: ['aac', 'opus', 'mp3', 'flac'],
        containers: ['mp4']
      }
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
            capabilities: sessionState.capabilities
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

  // Add subtitle tracks
  if (sessionState.subtitles.length > 0) {
    loadRequestData.media.tracks = sessionState.subtitles
      .filter((s) => s.url && s.format !== 'burn_in')
      .map((sub, i) => {
        const track = new cast.framework.messages.Track(
          i + 1,
          cast.framework.messages.TrackType.TEXT
        );
        track.trackContentId = appendToken(sub.url);
        track.trackContentType = 'text/vtt';
        track.subtype = cast.framework.messages.TextTrackType.SUBTITLES;
        track.name = sub.title || sub.language || `Track ${i + 1}`;
        track.language = sub.language || 'und';
        return track;
      });
  }

  return loadRequestData;
});

// ── SEEK Interceptor (Generation-Based) ────────────────────────

playerManager.setMessageInterceptor(cast.framework.messages.MessageType.SEEK, async (seekData) => {
  if (!sessionState.sessionId) {
    return seekData;
  }

  const targetTime = seekData.currentTime;
  // A seek must not resume a receiver the viewer paused. The reload below is
  // our own machinery, not a request to play.
  const wasPaused = playerManager.getPlayerState() === cast.framework.messages.PlayerState.PAUSED;

  // Show thumbnail preview during seek
  showThumbnailAtTime(targetTime);

  // Direct play: native seek via range requests, no server interaction needed
  if (sessionState.method === 'direct_play') {
    setTimeout(hideThumbnail, 500);
    return seekData;
  }

  // HLS: generation-based seek via Kobbler API
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

    if (resp.ok) {
      const data = await resp.json();
      seekGeneration = data.seek_generation;

      // Force reload with new generation URL via fresh LoadRequestData
      const newUrl = appendToken(appendGeneration(sessionState.playlistUrl, seekGeneration));
      const mediaInfo = playerManager.getMediaInformation();
      if (mediaInfo) {
        mediaInfo.contentUrl = newUrl;
        mediaInfo.contentId = newUrl;

        const loadRequest = new cast.framework.messages.LoadRequestData();
        loadRequest.media = mediaInfo;
        loadRequest.currentTime = data.start_time + (data.sub_segment_offset || 0);
        loadRequest.autoplay = !wasPaused;

        playerManager.load(loadRequest).catch((err) => {
          console.error('[Kobbler] Seek reload failed:', err);
        });
      }

      setTimeout(hideThumbnail, 500);
      // Return null to suppress CAF's default seek — we're doing our own reload
      return null;
    }
  } catch (err) {
    console.error('[Kobbler] Seek failed:', err);
  }

  setTimeout(hideThumbnail, 500);
  return seekData;
});

// ── Media Status Events ────────────────────────────────────────

playerManager.addEventListener(cast.framework.events.EventType.MEDIA_FINISHED, () => {
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
});

// ── Skip Segments ──────────────────────────────────────────────

function checkSkipSegments() {
  const mediaEl = document.querySelector('video');
  if (!mediaEl) return;

  const t = mediaEl.currentTime;
  const active = sessionState.skipSegments.find((s) => t >= s.start_seconds && t < s.end_seconds);

  if (active) {
    showSkipOverlay(active);
  } else {
    hideSkipOverlay();
  }
}

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
  if (!isNaN(skipTo)) {
    // Trigger a seek via the player manager
    const mediaEl = document.querySelector('video');
    if (mediaEl) {
      playerManager.seek(skipTo);
    }
  }
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
  const mediaEl = document.querySelector('video');
  if (!mediaEl) return;

  const t = mediaEl.currentTime;
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
  const trackId = payload.trackId;
  if (trackId === null) {
    // Disable subtitles
    playerManager.getQueueManager();
    const mediaInfo = playerManager.getMediaInformation();
    if (mediaInfo) {
      const editRequest = new cast.framework.messages.EditTracksInfoRequestData();
      editRequest.activeTrackIds = [];
      // Apply via player manager
    }
  }
  // For simplicity, sender manages subtitle selection via EDIT_TRACKS_INFO
}

async function handleAudioSwitch(payload) {
  if (!sessionState.sessionId) return;

  try {
    const resp = await fetch(
      appendToken(
        `${sessionState.baseUrl}/api/v1/admin/playback/sessions/${sessionState.sessionId}/switch-audio`
      ),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          audio_stream_index: payload.streamIndex,
          resume_time: payload.resumeTime
        })
      }
    );

    if (resp.ok) {
      const data = await resp.json();
      // Reload with new session data
      if (data.playlist_url) {
        sessionState.playlistUrl = data.playlist_url;
        // Re-load media
        const mediaInfo = playerManager.getMediaInformation();
        if (mediaInfo) {
          mediaInfo.contentUrl = appendToken(data.playlist_url);
          mediaInfo.contentId = appendToken(data.playlist_url);
        }
      }
    }
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
