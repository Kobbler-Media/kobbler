/**
 * Audiobook Cast sender flow.
 *
 * Transfers audiobook playback to a Cast device.
 * The receiver loads audio files directly (MP3/M4B via range requests),
 * handles file transitions, saves position, and broadcasts updates.
 */

import { createPlaybackToken, revokePlaybackToken } from '$lib/api/client';
import { getAudioPlayerState, pause, getFileIds } from '$lib/stores/audioplayer.svelte';
import { getCastState } from '$lib/stores/cast.svelte';
import { sendMessage } from '$lib/cast/sender';
import type { InitAudiobookPayload } from '$lib/cast/messages';

let audiobookCastToken: string | null = null;
let audiobookCasting = false;

export function isAudiobookCasting(): boolean {
  return audiobookCasting;
}

export function getAudiobookCastToken(): string | null {
  return audiobookCastToken;
}

/**
 * Start casting the current audiobook to the connected Cast device.
 */
export async function startAudiobookCast(): Promise<void> {
  const player = getAudioPlayerState();
  const cast = getCastState();

  if (!player.currentBook || !player.currentFileId || !cast.connected) return;

  // Pause local playback
  pause();

  try {
    const { token } = await createPlaybackToken(24);
    audiobookCastToken = token;

    const baseUrl = window.location.origin;
    const fIds = getFileIds();

    // Map chapters for the current file
    const chapters = player.unifiedChapters
      .filter((c) => c.file_id === player.currentFileId)
      .map((c) => ({
        index: c.chapter_index,
        title: c.title,
        startSeconds: c.start_seconds,
        endSeconds: c.end_seconds
      }));

    const payload: InitAudiobookPayload = {
      baseUrl,
      token,
      bookId: player.currentBook.id,
      fileIds: fIds,
      currentFileIndex: player.currentTrackIndex,
      currentTime: player.currentTime,
      chapters,
      duration: player.duration,
      metadata: {
        title: player.currentBook.title,
        author: player.currentBook.authors?.map((a) => a.name).join(', '),
        coverUrl: player.currentBook.cover_url
          ? `${baseUrl}${player.currentBook.cover_url}`
          : undefined
      }
    };

    sendMessage({ type: 'INIT_AUDIOBOOK', payload });
    audiobookCasting = true;
  } catch (err) {
    console.error('Failed to start audiobook cast:', err);
    if (audiobookCastToken) {
      revokePlaybackToken(audiobookCastToken).catch(() => {});
      audiobookCastToken = null;
    }
  }
}

/**
 * Stop audiobook casting and clean up.
 */
export function stopAudiobookCast(): void {
  audiobookCasting = false;
  if (audiobookCastToken) {
    revokePlaybackToken(audiobookCastToken).catch(() => {});
    audiobookCastToken = null;
  }
}
