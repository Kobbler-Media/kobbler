// Search logic combining client-side and API search (using MeiliSearch federated search)

import type { SearchResult, GroupedResults } from './types';
import type { UnifiedSearchResponse, SearchHit, TranscriptSearchResult } from '$lib/api/client';
import { searchAllTranscripts } from '$lib/api/client';
import { searchPages, searchCommands, searchSections, icons } from './registry';

// Parse prefix from query
export type SearchMode =
  'all' | 'commands' | 'authors' | 'series' | 'pages' | 'books' | 'transcripts' | 'chat';

export function parseQuery(rawQuery: string): { mode: SearchMode; query: string } {
  const trimmed = rawQuery.trim();
  if (trimmed.startsWith('>')) {
    return { mode: 'commands', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('@')) {
    return { mode: 'authors', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith(':')) {
    return { mode: 'series', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('/')) {
    return { mode: 'pages', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('#')) {
    return { mode: 'books', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('~')) {
    return { mode: 'transcripts', query: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('?')) {
    return { mode: 'chat', query: trimmed.slice(1).trim() };
  }
  return { mode: 'all', query: trimmed };
}

// Convert a SearchHit from the federated API response to a SearchResult.
function convertHit(hit: SearchHit): SearchResult | null {
  switch (hit.type) {
    case 'book': {
      // Format: "Author - Series Name #index" or "Author + Others - Series Name #index"
      const authorPart =
        hit.authors.length > 1 ? `${hit.authors[0]} + Others` : (hit.authors[0] ?? 'Unknown');
      let description = authorPart;
      if (hit.series_name) {
        const indexPart = hit.series_index != null ? ` #${hit.series_index}` : '';
        description += ` — ${hit.series_name}${indexPart}`;
      }
      const isIngest = hit.ingest_status != null;
      return {
        type: isIngest ? 'ingest' : 'book',
        id: hit.id,
        label: hit.title,
        description,
        coverUrl: hit.cover_url ?? undefined,
        isInLibrary: hit.is_in_library,
        wantStatus: hit.want_status,
        hasAudiobook: hit.has_audiobook,
        hasEbook: hit.has_ebook
      };
    }

    case 'author':
      return {
        type: 'author',
        id: hit.id,
        label: hit.name,
        description: `${hit.book_count} book${hit.book_count !== 1 ? 's' : ''}`,
        imageUrl: hit.image_url ?? undefined,
        isMonitored: hit.is_monitored
      };

    case 'series':
      return {
        type: 'series',
        id: hit.id,
        label: hit.name,
        description: `${hit.book_count} book${hit.book_count !== 1 ? 's' : ''}`,
        isMonitored: hit.is_monitored
      };

    case 'movie': {
      const directors = hit.directors ?? [];
      const parts: string[] = [];
      if (directors.length > 0) parts.push(directors.join(', '));
      if (hit.year) parts.push(`(${hit.year})`);
      return {
        type: 'movie',
        id: hit.id,
        label: hit.title,
        description: parts.join(' '),
        coverUrl: hit.poster_url ?? undefined,
        isInLibrary: hit.is_in_library,
        wantStatus: hit.want_status
      };
    }

    case 'tv_show': {
      return {
        type: 'tv_show',
        id: hit.id,
        label: hit.title,
        description: [hit.network, hit.year ? `(${hit.year})` : '', hit.status]
          .filter(Boolean)
          .join(' \u00B7 '),
        coverUrl: hit.poster_url ?? undefined,
        isMonitored: hit.is_monitored
      };
    }

    case 'tv_season': {
      return {
        type: 'tv_season',
        id: hit.id,
        label: hit.display_title,
        description: `${hit.episode_count} episode${hit.episode_count !== 1 ? 's' : ''}`,
        coverUrl: hit.poster_url ?? undefined,
        isMonitored: hit.is_monitored
      };
    }

    case 'tv_episode': {
      return {
        type: 'tv_episode',
        id: hit.id,
        label: hit.display_title,
        description: hit.episode_title ?? undefined,
        coverUrl: hit.still_url ?? undefined,
        isInLibrary: hit.is_in_library,
        wantStatus: hit.want_status
      };
    }

    case 'genre':
      // For now, skip genres in the command palette
      return null;

    case 'page':
      return {
        type: 'page',
        id: hit.id,
        label: hit.label,
        path: hit.path,
        icon: hit.icon
      };

    case 'command':
      return {
        type: 'command',
        id: hit.id,
        label: hit.label,
        action: hit.action,
        icon: hit.icon ?? icons.general
      };

    case 'transcript': {
      const tParts: string[] = [];
      if (hit.chapter_title) tParts.push(hit.chapter_title);
      if (hit.source_type === 'audio' && hit.start_seconds != null) {
        tParts.push(formatTranscriptTime(hit.start_seconds));
      }
      const tLocationInfo = tParts.length > 0 ? ` — ${tParts.join(' · ')}` : '';
      return {
        type: 'transcript' as const,
        id: hit.id,
        label: hit.text,
        description: `${hit.book_title}${tLocationInfo}`,
        path:
          hit.source_type === 'audio'
            ? `/library/books/${hit.book_id}/listen${hit.start_seconds != null ? `?t=${Math.floor(hit.start_seconds)}` : ''}`
            : `/library/books/${hit.book_id}`
      };
    }

    case 'iptv_channel':
      return {
        type: 'iptv_channel' as const,
        id: hit.id,
        label: hit.name,
        description:
          [hit.category, hit.source_count > 0 ? `${hit.source_count} sources` : null]
            .filter(Boolean)
            .join(' · ') || undefined,
        coverUrl: hit.icon_url ?? undefined,
        path: `/iptv/${hit.id}`
      };

    case 'livetv_channel':
      return {
        type: 'livetv_channel' as const,
        id: hit.id,
        label: hit.name,
        description:
          [hit.lcn, hit.network, hit.type_label].filter(Boolean).join(' · ') || undefined,
        coverUrl: hit.logo_url ?? undefined,
        path: `/livetv/channels?search=${encodeURIComponent(hit.name)}`
      };

    case 'livetv_guide':
      // Guide entries are excluded from the unified command palette for the same
      // reason as IPTV EPG — scheduled programmes swamp everything else.
      return null;

    case 'iptv_epg':
      // EPG entries are excluded from the unified command palette —
      // too many noisy results from scheduled programmes. EPG is still
      // searchable in the IPTV guide page.
      return null;

    case 'setting':
      return {
        type: 'setting' as const,
        id: hit.id,
        label: hit.label,
        description: `${hit.page_label} — ${hit.description}`,
        path: `${hit.page_path}#field=${hit.field_name}`,
        icon: 'settings'
      };

    default:
      return null;
  }
}

// Process federated search results into grouped results
function processFederatedResults(
  response: UnifiedSearchResponse,
  mode: SearchMode
): Partial<GroupedResults> {
  const results: Partial<GroupedResults> = {
    topHits: [],
    books: [],
    ingest: [],
    authors: [],
    series: [],
    movies: [],
    tv_shows: [],
    tv_seasons: [],
    tv_episodes: [],
    pages: [],
    commands: [],
    settings: [],
    externalSearch: [],
    transcripts: [],
    iptv_channels: [],
    livetv_channels: []
  };

  // Helper to check if result matches mode filter
  const matchesMode = (type: string): boolean => {
    if (mode === 'all') return true;
    if (mode === 'books' && (type === 'book' || type === 'ingest')) return true;
    if (mode === 'authors' && type === 'author') return true;
    if (mode === 'series' && type === 'series') return true;
    if (mode === 'pages' && (type === 'page' || type === 'setting')) return true;
    if (mode === 'commands' && type === 'command') return true;
    return false;
  };

  // Process top_hits - these are the best hit per type, already in relevance order
  for (const hit of response.top_hits) {
    const result = convertHit(hit);
    if (!result || !matchesMode(result.type)) continue;
    results.topHits!.push(result);
  }

  // Process remaining results into type groups
  for (const hit of response.remaining) {
    const result = convertHit(hit);
    if (!result || !matchesMode(result.type)) continue;

    switch (result.type) {
      case 'book':
        results.books!.push(result);
        break;
      case 'ingest':
        results.ingest!.push(result);
        break;
      case 'author':
        results.authors!.push(result);
        break;
      case 'series':
        results.series!.push(result);
        break;
      case 'movie':
        results.movies!.push(result);
        break;
      case 'tv_show':
        results.tv_shows!.push(result);
        break;
      case 'tv_season':
        results.tv_seasons!.push(result);
        break;
      case 'tv_episode':
        results.tv_episodes!.push(result);
        break;
      case 'page':
        results.pages!.push(result);
        break;
      case 'command':
        results.commands!.push(result);
        break;
      case 'setting':
        results.settings!.push(result);
        break;
      case 'transcript':
        results.transcripts!.push(result);
        break;
      case 'iptv_channel':
        results.iptv_channels!.push(result);
        break;
      case 'livetv_channel':
        results.livetv_channels!.push(result);
        break;
    }
  }

  return results;
}

// Client-side search for static items (fallback when MeiliSearch pages/commands are empty)
export function searchStatic(query: string, mode: SearchMode): Partial<GroupedResults> {
  const results: Partial<GroupedResults> = {};

  if (mode === 'all' || mode === 'pages') {
    const pageResults = searchPages(query);
    results.pages = pageResults.map((page) => ({
      type: 'page' as const,
      id: page.id,
      label: page.label,
      path: page.path,
      icon: page.icon
    }));

    const sectionResults = searchSections(query);
    results.sections = sectionResults.map((section) => ({
      type: 'section' as const,
      id: section.id,
      label: section.label,
      path: section.path
    }));
  }

  if (mode === 'all' || mode === 'commands') {
    const commandResults = searchCommands(query);
    results.commands = commandResults.map((cmd) => ({
      type: 'command' as const,
      id: cmd.id,
      label: cmd.label,
      action: cmd.action,
      icon: cmd.icon ?? icons.general
    }));
  }

  return results;
}

function formatTranscriptTime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function convertTranscriptHits(hits: TranscriptSearchResult[]): SearchResult[] {
  return hits.map((hit) => {
    const parts: string[] = [];
    if (hit.chapter_title) parts.push(hit.chapter_title);
    if (hit.source_type === 'audio' && hit.start_seconds != null) {
      parts.push(formatTranscriptTime(hit.start_seconds));
    }
    const locationInfo = parts.length > 0 ? ` — ${parts.join(' · ')}` : '';
    return {
      type: 'transcript' as const,
      id: hit.id,
      label: hit.text,
      description: `${hit.book_title}${locationInfo}`,
      path:
        hit.source_type === 'audio'
          ? `/library/books/${hit.book_id}/listen${hit.start_seconds != null ? `?t=${Math.floor(hit.start_seconds)}` : ''}`
          : `/library/books/${hit.book_id}`
    };
  });
}

// Combined search using MeiliSearch federated search
export async function search(
  rawQuery: string,
  apiSearch: (q: string, limit: number) => Promise<UnifiedSearchResponse>
): Promise<GroupedResults> {
  const { mode, query } = parseQuery(rawQuery);

  // Start with empty results
  const results: GroupedResults = {
    topHits: [],
    books: [],
    ingest: [],
    authors: [],
    series: [],
    movies: [],
    tv_shows: [],
    tv_seasons: [],
    tv_episodes: [],
    pages: [],
    commands: [],
    settings: [],
    sections: [],
    externalSearch: [],
    transcripts: [],
    iptv_channels: [],
    livetv_channels: []
  };

  // Transcript mode uses a separate API
  if (mode === 'transcripts') {
    if (!query) return results;
    try {
      const transcriptResults = await searchAllTranscripts(query);
      results.transcripts = convertTranscriptHits(transcriptResults);
    } catch (e) {
      console.error('Transcript search error:', e);
    }
    return results;
  }

  // If query is empty, just return static results (pages, commands, sections)
  if (!query) {
    const staticResults = searchStatic(query, mode);
    Object.assign(results, staticResults);
    return results;
  }

  // Fetch API results (federated search across all indexes)
  try {
    const apiResults = await apiSearch(query, 10);
    const federatedResults = processFederatedResults(apiResults, mode);
    Object.assign(results, federatedResults);

    // Fall back to client-side search for static items if MeiliSearch returned none
    // (e.g., if static index hasn't been synced yet)
    if ((mode === 'all' || mode === 'pages') && (!results.pages || results.pages.length === 0)) {
      const staticPages = searchStatic(query, 'pages');
      results.pages = staticPages.pages || [];
      results.sections = staticPages.sections || [];
    }

    if (
      (mode === 'all' || mode === 'commands') &&
      (!results.commands || results.commands.length === 0)
    ) {
      const staticCommands = searchStatic(query, 'commands');
      results.commands = staticCommands.commands || [];
    }
  } catch (e) {
    console.error('Search API error:', e);
    // Fall back to static results only
    const staticResults = searchStatic(query, mode);
    Object.assign(results, staticResults);
  }

  // Inject external search action when searching across all types
  if (mode === 'all' && query) {
    results.externalSearch = [
      {
        type: 'external_search',
        id: 'ext-search-discover',
        label: `Discover "${query}"`,
        description: 'Search metadata providers'
      }
    ];
  }

  return results;
}

// Get URL for a search result - uses full page views for entities
export function getResultUrl(result: SearchResult): string | null {
  switch (result.type) {
    case 'book':
      return `/library/books/${result.id}`;

    case 'ingest':
      return `/ingest-queue/${result.id}`;

    case 'author':
      return `/authors/${result.id}`;

    case 'series':
      return `/series/${result.id}`;

    case 'movie':
      return `/movies/${result.id}`;

    case 'tv_show':
      return `/tv/${result.id}`;

    case 'tv_season':
      return `/tv/seasons/${result.id}`;

    case 'tv_episode':
      return `/tv/episodes/${result.id}`;

    case 'page':
    case 'section':
    case 'setting':
    case 'transcript':
      return result.path ?? null;

    case 'command':
    case 'external_search':
      return null; // Commands and external searches execute actions, not navigate

    case 'iptv_channel':
    case 'livetv_channel':
      return result.path ?? null;
  }
}

// Flatten grouped results into a single array for keyboard navigation
// Top hits appear first (best hit per type in relevance order),
// followed by remaining results grouped by type
export function flattenResults(grouped: GroupedResults): SearchResult[] {
  const flat: SearchResult[] = [];

  // Top hits first - these are the best hit per type, already ordered by relevance
  flat.push(...grouped.topHits);

  // Then remaining results by type group
  const typeOrder: (keyof Omit<GroupedResults, 'topHits'>)[] = [
    'pages',
    'commands',
    'settings',
    'authors',
    'series',
    'books',
    'ingest',
    'movies',
    'tv_shows',
    'tv_seasons',
    'tv_episodes',
    'sections',
    'externalSearch',
    'transcripts',
    'iptv_channels',
    'livetv_channels'
  ];

  for (const key of typeOrder) {
    flat.push(...grouped[key]);
  }

  return flat;
}
