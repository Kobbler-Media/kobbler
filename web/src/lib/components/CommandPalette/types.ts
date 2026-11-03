// Command palette types

export type ResultType =
  | 'book'
  | 'ingest'
  | 'author'
  | 'series'
  | 'page'
  | 'command'
  | 'setting'
  | 'section'
  | 'movie'
  | 'tv_show'
  | 'tv_season'
  | 'tv_episode'
  | 'external_search'
  | 'transcript'
  | 'iptv_channel'
  | 'livetv_channel';

export interface SearchResult {
  type: ResultType;
  id: string;
  label: string;
  description?: string;
  icon?: string;
  path?: string;
  action?: string;
  coverUrl?: string;
  imageUrl?: string;
  isMonitored?: boolean;
  isInLibrary?: boolean;
  wantStatus?: string;
  hasAudiobook?: boolean;
  hasEbook?: boolean;
}

export interface PageItem {
  id: string;
  label: string;
  path: string;
  aliases: string[];
  icon: string;
}

export type MinRole = 'admin' | 'contributor';

export interface CommandItem {
  id: string;
  label: string;
  action: string;
  aliases: string[];
  icon?: string;
  /** If set, user must confirm before executing. */
  confirmMessage?: string;
  /** Omitted means every authenticated role may run it. */
  minRole?: MinRole;
}

export interface SectionItem {
  id: string;
  label: string;
  path: string;
  parent: string;
}

export interface GroupedResults {
  // Top hit per type, ordered by relevance (e.g., Author first when searching author name)
  topHits: SearchResult[];
  // Remaining results grouped by type
  books: SearchResult[];
  ingest: SearchResult[];
  authors: SearchResult[];
  series: SearchResult[];
  movies: SearchResult[];
  tv_shows: SearchResult[];
  tv_seasons: SearchResult[];
  tv_episodes: SearchResult[];
  pages: SearchResult[];
  commands: SearchResult[];
  settings: SearchResult[];
  sections: SearchResult[];
  externalSearch: SearchResult[];
  transcripts: SearchResult[];
  iptv_channels: SearchResult[];
  livetv_channels: SearchResult[];
}

// Re-export API response types
export type {
  UnifiedSearchResponse,
  UnifiedSearchBook,
  UnifiedSearchAuthor,
  UnifiedSearchSeries
} from '#lib/api/client.js';
