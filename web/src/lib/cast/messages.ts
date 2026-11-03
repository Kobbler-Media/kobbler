/**
 * Cast custom namespace message types.
 * Shared contract between sender and receiver.
 */

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
  capabilities: { video_codecs: string[]; audio_codecs: string[]; containers: string[] };
  metadata: {
    title: string;
    subtitle?: string;
    posterUrl?: string;
    backdropUrl?: string;
  };
};

export type AudiobookChapterInfo = {
  index: number;
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
  metadata: {
    title: string;
    author?: string;
    coverUrl?: string;
  };
};

export type CastMessage =
  | { type: 'INIT_SESSION'; payload: InitSessionPayload }
  | { type: 'INIT_AUDIOBOOK'; payload: InitAudiobookPayload }
  | { type: 'SUBTITLE_CHANGE'; payload: { trackId: string | null } }
  | { type: 'AUDIO_SWITCH'; payload: { streamIndex: number; resumeTime: number } }
  | { type: 'SKIP_TO'; payload: { timeSeconds: number } }
  | { type: 'TOKEN_REFRESH'; payload: { token: string } }
  | { type: 'STATUS_REQUEST' }
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

export type ReceiverStatus = {
  currentTime: number;
  duration: number;
  playing: boolean;
  volume: number;
  mediaId?: string | null;
  mediaType?: string | null;
  metadata?: { title?: string; subtitle?: string } | null;
  isAudiobook?: boolean;
};
