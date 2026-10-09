// Global PiP ownership store — holds video resources that must survive
// VideoPlayer unmount when the user enters native Picture-in-Picture.

import {
  destroyPlaybackSession,
  heartbeatSession,
  updateMoviePosition,
  updateEpisodePosition
} from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import type Hls from 'hls.js';

interface CreationParams {
  media_type: 'movie' | 'episode' | 'live' | 'recording';
  file_id: string;
}

export interface PipTransfer {
  videoElement: HTMLVideoElement;
  hlsInstance: Hls | null;
  overlayDiv: HTMLDivElement;
  session: PlaybackSessionResponse;
  creationParams: CreationParams;
  mediaId: string;
  seekOffset: number;
  audioContext: AudioContext | null;
}

// Module-level singleton state
let videoElement: HTMLVideoElement | null = null;
let hlsInstance: Hls | null = null;
let overlayDiv: HTMLDivElement | null = null;
let session: PlaybackSessionResponse | null = null;
let creationParams: CreationParams | null = null;
let mediaId: string = '';
let seekOffset: number = 0;
let audioContext: AudioContext | null = null;
let heartbeatInterval: ReturnType<typeof setInterval> | null = null;
let positionSaveInterval: ReturnType<typeof setInterval> | null = null;

function getDisplayTime(): number {
  if (!videoElement) return 0;
  return seekOffset + videoElement.currentTime;
}

async function savePosition() {
  if (!session || !mediaId || !creationParams) return;
  if (creationParams.media_type === 'live') return;
  const pos = Math.floor(getDisplayTime());
  if (pos <= 0) return;
  try {
    if (creationParams.media_type === 'movie') {
      await updateMoviePosition(mediaId, pos);
    } else if (creationParams.media_type === 'episode') {
      await updateEpisodePosition(mediaId, pos);
    }
  } catch (e) {
    console.warn('[PiP] Failed to save position:', e);
  }
}

function onLeavePip() {
  cleanupPip();
}

export function enterPip(transfer: PipTransfer) {
  // Clean up any existing PiP first
  if (videoElement) {
    cleanupPip();
  }

  videoElement = transfer.videoElement;
  hlsInstance = transfer.hlsInstance;
  overlayDiv = transfer.overlayDiv;
  session = transfer.session;
  creationParams = transfer.creationParams;
  mediaId = transfer.mediaId;
  seekOffset = transfer.seekOffset;
  audioContext = transfer.audioContext;

  // Hide the overlay so the app is usable underneath
  if (overlayDiv) {
    overlayDiv.style.display = 'none';
  }

  // Listen for PiP exit to clean up
  videoElement.addEventListener('leavepictureinpicture', onLeavePip, { once: true });

  // Keep the backend session alive (all types including IPTV live)
  if (session) {
    heartbeatInterval = setInterval(async () => {
      if (!session) return;
      const alive = await heartbeatSession(session.session_id);
      if (!alive) {
        console.warn('[PiP] Session lost during PiP');
        cleanupPip();
      }
    }, 120_000);

    // Position saving only for VOD (not live)
    if (creationParams?.media_type !== 'live') {
      positionSaveInterval = setInterval(() => savePosition(), 30_000);
    }
  }
}

export function cleanupPip() {
  // Save final position
  savePosition().catch(() => {});

  // Clear intervals
  if (heartbeatInterval) {
    clearInterval(heartbeatInterval);
    heartbeatInterval = null;
  }
  if (positionSaveInterval) {
    clearInterval(positionSaveInterval);
    positionSaveInterval = null;
  }

  // Remove PiP exit listener (if cleanup called externally, not from event)
  if (videoElement) {
    videoElement.removeEventListener('leavepictureinpicture', onLeavePip);
  }

  // Exit native PiP if still active
  if (document.pictureInPictureElement) {
    document.exitPictureInPicture?.().catch(() => {});
  }

  // Destroy backend session
  if (session) {
    destroyPlaybackSession(session.session_id).catch(() => {});
    session = null;
  }

  // Destroy HLS
  if (hlsInstance) {
    hlsInstance.destroy();
    hlsInstance = null;
  }

  // Close AudioContext
  if (audioContext) {
    audioContext.close().catch(() => {});
    audioContext = null;
  }

  // Stop and clear video element
  if (videoElement) {
    videoElement.pause();
    videoElement.removeAttribute('src');
    videoElement.load();
    videoElement = null;
  }

  // Remove the orphaned overlay div from body
  if (overlayDiv) {
    overlayDiv.remove();
    overlayDiv = null;
  }

  creationParams = null;
  mediaId = '';
  seekOffset = 0;
}

export function isPipActive(): boolean {
  return videoElement !== null;
}
