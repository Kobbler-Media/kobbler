/**
 * Reactive Cast state store (Svelte 5 runes).
 *
 * Tracks Cast SDK availability, connection state, device info,
 * and current media state on the receiver.
 */

import {
  initCastSdk,
  requestSession as sdkRequestSession,
  endSession as sdkEndSession,
  onStateChange,
  onMessage,
  sendMessage,
  loadMedia,
  isSdkReady,
  getDeviceName as sdkGetDeviceName
} from '#lib/cast/sender.js';
import { startAudiobookCast, stopAudiobookCast, isAudiobookCasting } from '#lib/cast/audiobook.js';
import { getAudioPlayerState } from '#lib/stores/audioplayer.svelte.js';
import type { InitSessionPayload, ReceiverStatus } from '#lib/cast/messages.js';

// ── State ──────────────────────────────────────────────────────

let castAvailable = $state(false);
let castConnected = $state(false);
let castDeviceName = $state<string | null>(null);
let castMediaState = $state<ReceiverStatus | null>(null);
let initialized = false;

// ── Accessor ───────────────────────────────────────────────────

export function getCastState() {
  return {
    get available() {
      return castAvailable;
    },
    get connected() {
      return castConnected;
    },
    get deviceName() {
      return castDeviceName;
    },
    get mediaState() {
      return castMediaState;
    }
  };
}

// ── Actions ────────────────────────────────────────────────────

/**
 * Initialize Cast SDK. Call once from the root layout.
 * Pass the receiver app ID from server config.
 */
export async function initCast(receiverAppId: string): Promise<void> {
  if (initialized) return;
  initialized = true;

  const ok = await initCastSdk(receiverAppId);
  castAvailable = ok;

  if (ok) {
    onStateChange((connected, deviceName, resumed) => {
      castConnected = connected;
      castDeviceName = deviceName;
      if (connected) {
        if (resumed) {
          // Browser reopened with an existing Cast session — query receiver for state
          sendMessage({ type: 'STATUS_REQUEST' });
        } else {
          // Fresh connection — auto-transfer audiobook if active (playing or paused)
          const audioPlayer = getAudioPlayerState();
          if (audioPlayer.isActive) {
            startAudiobookCast();
          }
        }
      } else {
        castMediaState = null;
        if (isAudiobookCasting()) {
          stopAudiobookCast();
        }
      }
    });

    onMessage((msg) => {
      if (msg.type === 'STATUS_RESPONSE') {
        castMediaState = msg.payload;
      } else if (msg.type === 'POSITION_UPDATE') {
        castMediaState = { ...msg.payload, volume: castMediaState?.volume ?? 1 };
      } else if (msg.type === 'TOKEN_REFRESH') {
        // Receiver self-refreshed its token — no action needed on sender side,
        // but log for debugging
        console.log('[Cast] Receiver refreshed token');
      } else if (msg.type === 'EPISODE_CHANGED') {
        // Update displayed title when receiver auto-advances
        if (castMediaState) {
          const ep = msg.payload;
          castMediaState = {
            ...castMediaState,
            mediaId: ep.episodeId,
            metadata: {
              title: ep.showTitle,
              subtitle: `S${ep.seasonIdentifier}E${ep.episodeIdentifier}${ep.title ? ' - ' + ep.title : ''}`
            }
          };
        }
      }
    });
  }
}

/**
 * Open the Cast device picker.
 */
export async function requestCastSession(): Promise<boolean> {
  if (!isSdkReady()) return false;
  return sdkRequestSession();
}

/**
 * End the current Cast session.
 */
export function endCastSession(): void {
  sdkEndSession();
  castConnected = false;
  castDeviceName = null;
  castMediaState = null;
}

/**
 * Load media on the Cast receiver.
 */
export function castLoadMedia(payload: InitSessionPayload): void {
  loadMedia(payload);
}

/**
 * Send a cast message to the receiver.
 */
export function castSendMessage(msg: Parameters<typeof sendMessage>[0]): void {
  sendMessage(msg);
}

/**
 * Get the friendly name of the connected Cast device.
 */
export function getCastDeviceName(): string | null {
  return sdkGetDeviceName();
}
