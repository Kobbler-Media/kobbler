/**
 * Cast custom namespace message types.
 * Shared contract between sender and receiver.
 */

import type { ClientCapabilities } from '#lib/utils/media.js';
import type {
  PlaybackAudioTrack,
  PlaybackSubtitleTrack,
  SkipSegment,
  VideoChapter
} from '#lib/types.js';

export const CAST_NAMESPACE = 'urn:x-cast:com.kobbler.cast';

export type InitSessionPayload = {
  baseUrl: string;
  token: string;
  sessionId: string;
  mediaId: string | null;
  mediaType: 'movie' | 'episode' | 'live' | 'recording';
  playlistUrl: string | null;
  directUrl: string | null;
  method: 'direct_play' | 'hls';
  duration: number;
  subtitles: PlaybackSubtitleTrack[];
  audioTracks: PlaybackAudioTrack[];
  thumbnailsUrl: string | null;
  chapters: VideoChapter[];
  skipSegments: SkipSegment[];
  capabilities: ClientCapabilities;
  startOffset: number;
  metadata: {
    title: string;
    subtitle?: string;
    posterUrl?: string;
    backdropUrl?: string;
  };
};

export type AudiobookChapterInfo = {
  index: number;
  fileIndex?: number;
  title: string | null;
  startSeconds: number;
  endSeconds: number;
};

export type InitAudiobookPayload = {
  baseUrl: string;
  token: string;
  bookId: string;
  fileIds: string[];
  currentFileIndex: number;
  currentTime: number;
  chapters: AudiobookChapterInfo[];
  duration: number;
  fileDurations?: number[];
  playbackRate?: number;
  metadata: {
    title: string;
    author?: string;
    coverUrl?: string;
  };
};

export type InitTtsPayload = {
  url: string;
  title: string;
  subtitle?: string;
  coverUrl?: string;
  startMs?: number;
};

export type CastMessage =
  | { type: 'INIT_SESSION'; payload: InitSessionPayload }
  | { type: 'INIT_AUDIOBOOK'; payload: InitAudiobookPayload }
  | { type: 'INIT_TTS'; payload: InitTtsPayload }
  | { type: 'TTS_STOP' }
  | { type: 'TTS_FINISHED' }
  | { type: 'AUDIOBOOK_SEEK'; payload: { fileIndex: number; time: number } }
  | { type: 'AUDIOBOOK_RATE'; payload: { rate: number } }
  | { type: 'SUBTITLE_CHANGE'; payload: { trackId: number | null } }
  | { type: 'AUDIO_SWITCH'; payload: { streamIndex: number } }
  | { type: 'TOKEN_REFRESH'; payload: { token: string } }
  | { type: 'STATUS_REQUEST' }
  | { type: 'CAPABILITIES_REQUEST' }
  | { type: 'CAPABILITIES'; payload: ClientCapabilities }
  | { type: 'TRACKS_REQUEST' }
  | { type: 'TRACKS'; payload: ReceiverTracks }
  | { type: 'SKIP_SEGMENT'; payload: { segmentType: string; endSeconds: number } | null }
  | { type: 'STATUS_RESPONSE'; payload: ReceiverStatus }
  | { type: 'ENRICHMENT_UPDATE'; payload: EnrichmentPayload }
  | {
      type: 'POSITION_UPDATE';
      payload: { currentTime: number; duration: number; playing: boolean };
    }
  | {
      type: 'EPISODE_CHANGED';
      payload: {
        episodeId: string;
        title: string | null;
        seasonIdentifier: string;
        episodeIdentifier: string;
        showTitle: string;
      };
    }
  | {
      type: 'AUDIOBOOK_FILE_CHANGED';
      payload: { fileIndex: number; fileId: string };
    }
  | { type: 'MEDIA_FINISHED' }
  | { type: 'SESSION_EXPIRED' };

export type EnrichmentPayload = {
  duration?: number;
  subtitles?: PlaybackSubtitleTrack[];
  audioTracks?: PlaybackAudioTrack[];
  skipSegments?: SkipSegment[];
  thumbnailsUrl?: string | null;
};

export type ReceiverTracks = {
  subtitles: Array<{
    id: number;
    language: string | null;
    languageName: string | null;
    title: string | null;
    forced: boolean;
    hearingImpaired: boolean;
  }>;
  activeSubtitleId: number | null;
  audioTracks: Array<{
    streamIndex: number;
    language: string | null;
    languageName: string | null;
    title: string | null;
    codec: string | null;
    channels: number | null;
  }>;
  activeAudioIndex: number | null;
};

export type ReceiverStatus = {
  currentTime: number;
  duration: number;
  playing: boolean;
  volume: number;
  mediaId?: string | null;
  mediaType?: string | null;
  metadata?: { title?: string; subtitle?: string } | null;
  isAudiobook?: boolean;
  isTts?: boolean;
};
