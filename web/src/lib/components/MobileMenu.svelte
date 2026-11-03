<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import NavGroup from './NavGroup.svelte';
  import { toggleGroup, isGroupExpanded } from '#lib/stores/sidebar.svelte.js';
  import { getStatsState, fetchStats } from '#lib/stores/stats.svelte.js';
  import { getLivetvState, fetchLivetvTuners } from '#lib/stores/livetv.svelte.js';
  import TunerMeter from './livetv/TunerMeter.svelte';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import { getFormatFilter, setFormatFilter } from '#lib/stores/format-filter.svelte.js';
  import { logout } from '#lib/services/data.svelte.js';
  import { goto } from '$app/navigation';
  import { registerOverlay, unregisterOverlay } from '#lib/stores/ui.svelte.js';
  import { PUBLIC_DEV_MODE } from '$app/env/public';

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  let filterQuery = $state('');
  const isDevMode = PUBLIC_DEV_MODE === '1';
  const statsState = getStatsState();
  const stats = $derived(statsState.stats);
  const livetv = getLivetvState();
  const authState = getAuthState();
  const formatFilter = $derived(getFormatFilter());

  onMount(() => {
    fetchStats();
    fetchLivetvTuners();
  });

  $effect(() => {
    if (open) {
      registerOverlay('mobile-menu', onClose);
      return () => unregisterOverlay('mobile-menu');
    }
  });

  // Alias mappings for fuzzy search (same as Sidebar)
  const aliases: Record<string, string[]> = {
    ingest: ['import', 'upload', 'add'],
    books: ['library', 'collection'],
    audiobook: ['audiobook', 'listen', 'narrator'],
    logs: ['activity', 'history'],
    proxy: ['kobo', 'sync'],
    'chat logs': ['chat', 'bot', 'turn', 'llm', 'assistant', 'debug'],
    devices: ['kobo', 'reader', 'ereader'],
    tokens: ['api', 'key', 'auth'],
    general: ['config', 'configuration', 'system'],
    audio: ['tts', 'text-to-speech', 'speech', 'transcription', 'whisper'],
    voice: ['tts', 'voice', 'preset', 'clone', 'emotion', 'speech'],
    metadata: ['info', 'data'],
    subtitles: ['subtitle', 'opensubtitles', 'captions', 'srt', 'vtt'],
    search: ['meili', 'meilisearch', 'index', 'indexer', 'find'],
    integrations: ['connect', 'external'],
    notifications: ['notify', 'push', 'pushover', 'plex', 'alert'],
    jobs: ['tasks', 'background', 'scheduled', 'queue', 'history'],
    'job concurrency': ['parallelism', 'workers', 'threads'],
    backup: ['export', 'restore', 'save'],
    obtain: ['download', 'kobload', 'wanted', 'ingestion', 'automation'],
    streaming: ['debrid', 'stremio', 'stream', 'real-debrid', 'torbox', 'cache'],
    cast: ['chromecast', 'google tv', 'casting', 'receiver', 'airplay'],
    'media-roots': ['root', 'folder', 'library', 'scan', 'media'],
    movies: ['movie', 'film', 'cinema', 'video'],
    tv: ['television', 'show', 'series', 'episode'],
    sports: ['league', 'motorsport', 'motogp', 'race', 'racing', 'formula'],
    '/livetv': ['live', 'tv', 'tuner', 'hdhomerun', 'antenna', 'epg', 'guide', 'broadcast'],
    '/livetv/channels': ['channels', 'lineup', 'scan', 'curate', 'mux', 'lcn', 'tuner'],
    '/livetv/recordings': ['dvr', 'record', 'recording', 'tuner'],
    '/livetv/scheduled': ['scheduled', 'series rule', 'upcoming', 'dvr'],
    '/livetv/on-now': ['on now', 'watch', 'suggestions', 'recommended', 'live'],
    '/livetv/series-rules': ['series rule', 'rules', 'season pass', 'dvr', 'record'],
    '/settings/livetv': ['tuner', 'hdhomerun', 'dvr', 'ad detection', 'buffer', 'timeshift'],
    '/settings/diagnostics': ['playback', 'reports', 'errors', 'failures', 'crash', 'client'],
    '/iptv': ['iptv', 'playlist', 'm3u', 'xtream', 'home'],
    '/iptv/channels': ['channels', 'stream', 'iptv'],
    '/iptv/recordings': ['recording', 'iptv'],
    people: ['person', 'actor', 'director', 'cast', 'crew'],
    calendar: ['upcoming', 'releases', 'schedule'],
    discover: [
      'find',
      'search',
      'add',
      'new',
      'provider',
      'tmdb',
      'streaming services',
      'netflix',
      'region'
    ],
    migration: ['sonarr', 'radarr', 'import', 'migrate'],
    profile: ['account', 'avatar', 'password', 'user']
  };

  function matchesFilter(item: NavItem, query: string): boolean {
    if (!query) return true;
    const q = query.toLowerCase().trim();
    if (item.label.toLowerCase().includes(q)) return true;
    const entries = aliases[item.href] ?? aliases[item.label.toLowerCase()] ?? [];
    return entries.some((alias) => alias.includes(q));
  }

  // NavItem type for filtering
  type NavItem = {
    href: string;
    label: string;
    icon: string;
    count?: number;
    badge?: number;
    minRole?: 'admin' | 'contributor';
  };

  function hasRole(minRole?: 'admin' | 'contributor'): boolean {
    if (!minRole) return true;
    if (minRole === 'admin') return authState.isAdmin;
    if (minRole === 'contributor') return authState.canManageContent;
    return false;
  }

  // Filter items based on query and role
  function filterItems(items: NavItem[]): NavItem[] {
    const roleFiltered = items.filter((item) => hasRole(item.minRole));
    if (!filterQuery.trim()) return roleFiltered;
    return roleFiltered.filter((item) => matchesFilter(item, filterQuery));
  }

  // Navigation structure with dynamic counts (same as Sidebar)
  const libraryItems = $derived([
    {
      href: '/library',
      label: 'Books',
      icon: 'M6 4h12a2 2 0 0 1 2 2v12h-2V6H6V4zm0 4h10a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2zm0 2v8h10v-8H6z',
      count: stats.books_count
    },
    {
      href: '/series',
      label: 'Series',
      icon: 'M3 7h18v2H3V7zm2 4h14v2H5v-2zm-2 4h18v2H3v-2z',
      count: stats.series_count
    },
    {
      href: '/authors',
      label: 'Authors',
      icon: 'M12 4a3 3 0 1 1 0 6 3 3 0 0 1 0-6zm-6 14a6 6 0 0 1 12 0H6z',
      count: stats.authors_count
    },
    {
      href: '/shelves',
      label: 'Shelves',
      icon: 'M4 4h16v2H4V4zm0 4h16v3H4V8zm0 5h16v3H4v-3zm0 5h16v2H4v-2z',
      count: stats.shelves_count
    }
  ]);

  const mediaItems = $derived([
    {
      href: '/movies',
      label: 'Movies',
      icon: 'M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z',
      count: stats.movies_count
    },
    {
      href: '/tv',
      label: 'TV Shows',
      icon: 'M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z',
      count: stats.tv_shows_count
    },
    {
      href: '/sports',
      label: 'Sports',
      icon: 'M19 5h-2V3H7v2H5c-1.1 0-2 .9-2 2v1c0 2.55 1.92 4.63 4.39 4.94.63 1.5 1.98 2.63 3.61 2.96V19H7v2h10v-2h-4v-3.1c1.63-.33 2.98-1.46 3.61-2.96C19.08 12.63 21 10.55 21 8V7c0-1.1-.9-2-2-2zM5 8V7h2v3.82C5.84 10.4 5 9.3 5 8zm14 0c0 1.3-.84 2.4-2 2.82V7h2v1z',
      count: stats.sports_count
    },
    {
      href: '/people',
      label: 'People',
      icon: 'M16 11c1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3 1.34 3 3 3zm-8 0c1.66 0 3-1.34 3-3S9.66 5 8 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 2 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z',
      count: stats.people_count
    },
    {
      href: '/calendar',
      label: 'Calendar',
      icon: 'M19 3h-1V1h-2v2H8V1H6v2H5c-1.11 0-1.99.9-1.99 2L3 19c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V8h14v11zM9 10H7v2h2v-2zm4 0h-2v2h2v-2zm4 0h-2v2h2v-2z'
    }
  ]);

  const livetvItems = $derived([
    {
      href: '/livetv',
      label: 'Guide',
      icon: 'M20 3h-1V1h-2v2H7V1H5v2H4c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 18H4V8h16v13z'
    },
    {
      href: '/livetv/channels',
      label: 'Channels',
      icon: 'M21 6h-7.59l3.29-3.29L16 2l-4 4-4-4-.71.71L10.59 6H3c-1.1 0-2 .89-2 2v12c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2V8c0-1.11-.9-2-2-2zm0 14H3V8h18v12z'
    },
    {
      href: '/livetv/recordings',
      label: 'Recordings',
      icon: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 15c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5z'
    },
    {
      href: '/livetv/scheduled',
      label: 'Scheduled',
      icon: 'M11.99 2C6.47 2 2 6.48 2 12s4.47 10 9.99 10C17.52 22 22 17.52 22 12S17.52 2 11.99 2zM12 20c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8zm.5-13H11v6l5.25 3.15.75-1.23-4.5-2.67z'
    },
    {
      href: '/livetv/on-now',
      label: 'On now',
      icon: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.41 0-8-3.59-8-8s3.59-8 8-8 8 3.59 8 8-3.59 8-8 8zm-2-11.5 6 3.5-6 3.5z'
    },
    {
      href: '/livetv/series-rules',
      label: 'Series rules',
      icon: 'M3 5h18v2H3V5zm0 6h12v2H3v-2zm0 6h18v2H3v-2zm14.5-6.5 3 3 4-4-1.4-1.4-2.6 2.6-1.6-1.6-1.4 1.4z'
    }
  ]);

  const liveItems = $derived([
    {
      href: '/iptv',
      label: 'Home',
      icon: 'M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z'
    },
    {
      href: '/iptv/channels',
      label: 'Channels',
      icon: 'M17 10.5V7c0-.55-.45-1-1-1H4c-.55 0-1 .45-1 1v10c0 .55.45 1 1 1h12c.55 0 1-.45 1-1v-3.5l4 4v-11l-4 4zM14 17H5V7h9v10z'
    },
    {
      href: '/iptv/recordings',
      label: 'Recordings',
      icon: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 14.5v-9l6 4.5-6 4.5z'
    }
  ]);

  const activityItems = $derived([
    {
      href: '/notifications',
      label: 'Notifications',
      icon: 'M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z'
    },
    {
      href: '/downloads',
      label: 'Downloads',
      icon: 'M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z',
      count: stats.downloads_active,
      minRole: 'contributor' as const
    },
    {
      href: '/ingest-queue',
      label: 'Ingest',
      icon: 'M4 6h16v2H4V6zm0 5h16v2H4v-2zm0 5h16v2H4v-2z',
      badge: stats.ingest_pending,
      minRole: 'contributor' as const
    },
    {
      href: '/activity',
      label: 'Logs',
      icon: 'M4 4h16v2H4V4zm0 6h10v2H4v-2zm0 6h16v2H4v-2z'
    },
    {
      href: '/proxy-logs',
      label: 'Proxy',
      icon: 'M3 3h18v2H3V3zm0 4h12v2H3V7zm0 4h18v2H3v-2zm0 4h12v2H3v-2zm0 4h18v2H3v-2z'
    },
    {
      href: '/chat-logs',
      label: 'Chat Logs',
      icon: 'M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-9 9H9V9h2v2zm4 0h-2V9h2v2z',
      minRole: 'admin' as const
    },
    {
      href: '/jobs',
      label: 'Jobs',
      icon: 'M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V5h14v14zM7 10h2v7H7zm4-3h2v10h-2zm4 6h2v4h-2z'
    }
  ]);

  const settingsItems: NavItem[] = [
    // Core
    {
      href: '/settings/general',
      label: 'General',
      icon: 'M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.49.49 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 0 0-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96a.49.49 0 0 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.07.62-.07.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z',
      minRole: 'admin'
    },
    {
      href: '/settings/media-roots',
      label: 'Media Roots',
      icon: 'M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm-6 12H4V8h16v10h-6zm-2-6l-4 4h3v2h2v-2h3l-4-4z',
      minRole: 'admin'
    },
    // Access
    {
      href: '/settings/users',
      label: 'Users',
      icon: 'M16 11c1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3 1.34 3 3 3zm-8 0c1.66 0 3-1.34 3-3S9.66 5 8 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 2 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z',
      minRole: 'admin'
    },
    {
      href: '/settings/chat-identities',
      label: 'Chat Identities',
      icon: 'M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-2 12H6v-2h12v2zm0-3H6V9h12v2zm0-3H6V6h12v2z',
      minRole: 'admin'
    },
    {
      href: '/settings/chat-memory',
      label: 'Chat Memory',
      icon: 'M13 3a9 9 0 0 0-9 9H1l3.89 3.89.07.14L9 12H6a7 7 0 1 1 2.05 4.95l-1.42 1.42A9 9 0 1 0 13 3zm-1 5v5l4.28 2.54.72-1.21-3.5-2.08V8H12z'
    },
    {
      href: '/settings/devices',
      label: 'Devices',
      icon: 'M7 2h10v2H7V2zm-2 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zm0 2v10h14V8H5z'
    },
    {
      href: '/settings/sessions',
      label: 'Sessions',
      icon: 'M17 1H7c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-2-2-2zm0 18H7V5h10v14zm-5-1.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z'
    },
    {
      href: '/settings/stremio-history',
      label: 'Stremio History',
      icon: 'M13 3a9 9 0 0 0-9 9H1l3.89 3.89.07.14L9 12H6a7 7 0 1 1 2.05 4.95l-1.42 1.42A9 9 0 1 0 13 3zm-1 5v5l4.28 2.54.72-1.21-3.5-2.08V8H12z'
    },
    {
      href: '/settings/watch-history',
      label: 'Watch History',
      icon: 'M13 3a9 9 0 0 0-9 9H1l3.89 3.89.07.14L9 12H6a7 7 0 1 1 2.05 4.95l-1.42 1.42A9 9 0 1 0 13 3zm-1 5v5l4.28 2.54.72-1.21-3.5-2.08V8H12z'
    },
    {
      href: '/settings/tokens',
      label: 'Tokens',
      icon: 'M12.65 10C11.83 7.67 9.61 6 7 6c-3.31 0-6 2.69-6 6s2.69 6 6 6c2.61 0 4.83-1.67 5.65-4H17v4h4v-4h2v-4H12.65zM7 14c-1.1 0-2-.9-2-2s.9-2 2-2 2 .9 2 2-.9 2-2 2z',
      minRole: 'admin'
    },
    {
      href: '/settings/kobo',
      label: 'Kobo Proxy',
      icon: 'M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 18H6V4h2v8l2.5-1.5L13 12V4h5v16z',
      minRole: 'admin'
    },
    // Media
    {
      href: '/settings/metadata',
      label: 'Metadata',
      icon: 'M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zM6 20V4h7v5h5v11H6z',
      minRole: 'admin'
    },
    {
      href: '/settings/subtitles',
      label: 'Subtitles',
      icon: 'M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zM4 18V6h16v12H4zm2-6h2v2H6v-2zm0 3h8v2H6v-2zm10 0h2v2h-2v-2zm-6-3h8v2h-8v-2z',
      minRole: 'admin'
    },
    {
      href: '/settings/audio',
      label: 'Audio & Speech',
      icon: 'M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z',
      minRole: 'admin'
    },
    {
      href: '/settings/voice',
      label: 'Voice Presets',
      icon: 'M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3zm-1-9c0-.55.45-1 1-1s1 .45 1 1v6c0 .55-.45 1-1 1s-1-.45-1-1V5zm6 6c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z'
    },
    // Playback
    {
      href: '/settings/streaming',
      label: 'Streaming',
      icon: 'M21 3H3c-1.11 0-2 .89-2 2v12a2 2 0 0 0 2 2h5v2h8v-2h5c1.1 0 1.99-.9 1.99-2L23 5a2 2 0 0 0-2-2zm0 14H3V5h18v12zm-5-6-7 4V7l7 4z',
      minRole: 'admin'
    },
    {
      href: '/settings/cast',
      label: 'Cast',
      icon: 'M1 18v3h3c0-1.66-1.34-3-3-3zm0-4v2c2.76 0 5 2.24 5 5h2c0-3.87-3.13-7-7-7zm0-4v2c4.97 0 9 4.03 9 9h2c0-6.08-4.93-11-11-11zm20-7H3c-1.1 0-2 .9-2 2v3h2V5h18v14h-7v2h7c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z',
      minRole: 'admin'
    },
    {
      href: '/settings/processing',
      label: 'Processing',
      icon: 'M17 16l-4-4V8.82C14.16 8.4 15 7.3 15 6c0-1.66-1.34-3-3-3S9 4.34 9 6c0 1.3.84 2.4 2 2.82V12l-4 4H3v5h5v-3.05l4-4.2 4 4.2V21h5v-5h-4z',
      minRole: 'admin'
    },
    {
      href: '/settings/diagnostics',
      label: 'Playback Reports',
      icon: 'M12 2L1 21h22L12 2zm1 15h-2v-2h2v2zm0-4h-2V9h2v4z',
      minRole: 'admin'
    },
    {
      href: '/settings/livetv',
      label: 'Live TV',
      icon: 'M21 6h-7.59l3.29-3.29L16 2l-4 4-4-4-.71.71L10.59 6H3c-1.1 0-2 .89-2 2v12c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2V8c0-1.11-.9-2-2-2zm0 14H3V8h18v12zM9 10v8l7-4-7-4z',
      minRole: 'admin'
    },
    {
      href: '/settings/iptv',
      label: 'IPTV',
      icon: 'M17 10.5V7c0-.55-.45-1-1-1H4c-.55 0-1 .45-1 1v10c0 .55.45 1 1 1h12c.55 0 1-.45 1-1v-3.5l4 4v-11l-4 4zM14 17H5V7h9v10z',
      minRole: 'admin'
    },
    // Services
    {
      href: '/settings/search',
      label: 'Search & AI',
      icon: 'M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z',
      minRole: 'admin'
    },
    {
      href: '/settings/obtain',
      label: 'Obtain',
      icon: 'M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM17 13l-5 5-5-5h3V9h4v4h3z',
      minRole: 'admin'
    },
    {
      href: '/settings/discover',
      label: 'Discover',
      icon: 'M12 10.9c-.61 0-1.1.49-1.1 1.1s.49 1.1 1.1 1.1c.61 0 1.1-.49 1.1-1.1s-.49-1.1-1.1-1.1zM12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm2.19 12.19L6 18l3.81-8.19L18 6l-3.81 8.19z',
      minRole: 'admin'
    },
    // Automation
    {
      href: '/settings/integrations',
      label: 'Integrations',
      icon: 'M17 7h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1 0 1.43-.98 2.63-2.31 2.98l1.46 1.46C20.88 15.61 22 13.95 22 12c0-2.76-2.24-5-5-5zm-1 4h-2.19l2 2H16v-2zM2 4.27l3.11 3.11C3.29 8.12 2 9.91 2 12c0 2.76 2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1 0-1.59 1.21-2.9 2.76-3.07L8.73 11H8v2h2.73L13 15.27V17h1.73l4.01 4L20 19.74 3.27 3 2 4.27z',
      minRole: 'admin'
    },
    {
      href: '/settings/notifications',
      label: 'Notifications',
      icon: 'M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z',
      minRole: 'admin'
    },
    {
      href: '/settings/jobs',
      label: 'Job Concurrency',
      icon: 'M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V5h14v14zM7 10h2v7H7zm4-3h2v10h-2zm4 6h2v4h-2z',
      minRole: 'admin'
    },
    // System
    {
      href: '/settings/backup',
      label: 'Backup',
      icon: 'M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z',
      minRole: 'admin'
    },
    {
      href: '/settings/migration',
      label: 'Migration',
      icon: 'M6.99 11L3 15l3.99 4v-3H14v-2H6.99v-3zM21 9l-3.99-4v3H10v2h7.01v3L21 9z',
      minRole: 'admin'
    }
  ];

  const devItems = [
    {
      href: '/dev-tools',
      label: 'Dev Tools',
      icon: 'M22.7 19l-9.1-9.1c.9-2.3.4-5-1.5-6.9-2-2-5-2.4-7.4-1.3L9 6 6 9 1.6 4.7C.4 7.1.9 10.1 2.9 12.1c1.9 1.9 4.6 2.4 6.9 1.5l9.1 9.1c.4.4 1 .4 1.4 0l2.3-2.3c.5-.4.5-1.1.1-1.4z'
    }
  ];

  // Filtered items
  const filteredLibraryItems = $derived(filterItems(libraryItems));
  const filteredMediaItems = $derived(
    filterItems(authState.isKids ? mediaItems.filter((i) => i.href !== '/sports') : mediaItems)
  );
  // The server refuses live and IPTV routes for a Kids profile; no point offering them.
  const filteredLivetvItems = $derived(authState.isKids ? [] : filterItems(livetvItems));
  const filteredLiveItems = $derived(authState.isKids ? [] : filterItems(liveItems));
  const filteredActivityItems = $derived(
    filterItems(
      authState.isKids ? activityItems.filter((i) => i.href !== '/activity') : activityItems
    )
  );
  const filteredSettingsItems = $derived(filterItems(settingsItems));
  const filteredDevItems = $derived(isDevMode ? filterItems(devItems) : []);
  const isFiltering = $derived(filterQuery.trim().length > 0);

  function isHomeActive(): boolean {
    return page.url.pathname === '/';
  }

  const avatarLetter = $derived(authState.email ? authState.email.charAt(0).toUpperCase() : '?');

  async function handleLogout() {
    onClose();
    await logout();
    goto('/', { replace: true });
  }

  // Close menu and clear filter only when an actual nav link is clicked
  function handleNavClick(e: MouseEvent) {
    const target = (e.target as HTMLElement).closest('a[href]');
    if (!target) return;
    filterQuery = '';
    onClose();
  }
</script>

{#if open}
  <button class="menu-backdrop" type="button" aria-label="Close menu" onclick={onClose}></button>
  <div class="menu-drawer">
    <div class="menu-header">
      <span class="menu-brand">Kobbler</span>
      <button class="close-btn" type="button" aria-label="Close menu" onclick={onClose}>
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
          <path
            d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
          ></path>
        </svg>
      </button>
    </div>

    <nav class="menu-nav" role="presentation" onclick={handleNavClick} onkeydown={() => {}}>
      <!-- Home link -->
      <a href="/" class="home-link" class:active={isHomeActive()} title="Home">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
          <path d="M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z"></path>
        </svg>
        <span>Home</span>
      </a>

      <!-- Dashboard link -->
      <a
        href="/dashboard"
        class="home-link"
        class:active={page.url.pathname === '/dashboard'}
        title="Dashboard"
      >
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
          <path d="M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z"></path>
        </svg>
        <span>Dashboard</span>
      </a>

      <!-- Filter input -->
      <div
        class="sidebar-filter"
        role="presentation"
        onclick={(e) => e.stopPropagation()}
        onkeydown={() => {}}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="filter-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          type="text"
          class="filter-input"
          placeholder="Filter..."
          bind:value={filterQuery}
          autocomplete="off"
        />
        {#if filterQuery}
          <button
            class="filter-clear"
            type="button"
            onclick={() => (filterQuery = '')}
            aria-label="Clear filter"
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
              <path
                d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
              ></path>
            </svg>
          </button>
        {/if}
      </div>

      <div class="menu-divider"></div>

      <!-- Navigation groups -->
      {#if filteredLibraryItems.length > 0}
        <NavGroup
          title="Library"
          items={filteredLibraryItems}
          expanded={isFiltering || isGroupExpanded('library')}
          collapsed={false}
          onToggle={() => toggleGroup('library')}
        >
          {#snippet headerExtra()}
            <span
              class="format-chips"
              role="presentation"
              onclick={(e) => e.stopPropagation()}
              onkeydown={() => {}}
            >
              <button
                class="fmt-chip"
                class:active={formatFilter === 'all'}
                type="button"
                title="All formats"
                onclick={() => setFormatFilter('all')}
              >
                <svg viewBox="0 0 24 24" width="10" height="10" fill="currentColor">
                  <path
                    d="M4 8h4V4H4v4zm6 12h4v-4h-4v4zm-6 0h4v-4H4v4zm0-6h4v-4H4v4zm6 0h4v-4h-4v4zm6-10v4h4V4h-4zm-6 4h4V4h-4v4zm6 6h4v-4h-4v4zm0 6h4v-4h-4v4z"
                  ></path>
                </svg>
              </button>
              <button
                class="fmt-chip"
                class:active={formatFilter === 'ebook'}
                type="button"
                title="Ebooks"
                onclick={() => setFormatFilter(formatFilter === 'ebook' ? 'all' : 'ebook')}
              >
                <span class="material-icons-outlined">auto_stories</span>
              </button>
              <button
                class="fmt-chip"
                class:active={formatFilter === 'audiobook'}
                type="button"
                title="Audiobooks"
                onclick={() => setFormatFilter(formatFilter === 'audiobook' ? 'all' : 'audiobook')}
              >
                <span class="material-icons-outlined">headphones</span>
              </button>
            </span>
          {/snippet}
        </NavGroup>
      {/if}

      {#if filteredMediaItems.length > 0}
        <NavGroup
          title="Media"
          items={filteredMediaItems}
          expanded={isFiltering || isGroupExpanded('media')}
          collapsed={false}
          onToggle={() => toggleGroup('media')}
        />
      {/if}

      {#if filteredLivetvItems.length > 0}
        <NavGroup
          title="Live TV"
          items={filteredLivetvItems}
          expanded={isFiltering || isGroupExpanded('livetv')}
          collapsed={false}
          onToggle={() => toggleGroup('livetv')}
        >
          {#snippet headerExtra()}
            {#if livetv.tunerCount > 0}
              <TunerMeter tunerCount={livetv.tunerCount} leasesInUse={livetv.leasesInUse} compact />
            {/if}
          {/snippet}
        </NavGroup>
      {/if}

      {#if filteredLiveItems.length > 0}
        <NavGroup
          title="IPTV"
          items={filteredLiveItems}
          expanded={isFiltering || isGroupExpanded('live')}
          collapsed={false}
          onToggle={() => toggleGroup('live')}
        />
      {/if}

      {#if filteredActivityItems.length > 0}
        <NavGroup
          title="Activity"
          items={filteredActivityItems}
          expanded={isFiltering || isGroupExpanded('activity')}
          collapsed={false}
          onToggle={() => toggleGroup('activity')}
        />
      {/if}

      {#if filteredSettingsItems.length > 0}
        <NavGroup
          title="Settings"
          items={filteredSettingsItems}
          expanded={isFiltering || isGroupExpanded('settings')}
          collapsed={false}
          onToggle={() => toggleGroup('settings')}
        />
      {/if}

      {#if filteredDevItems.length > 0}
        <NavGroup
          title="Development"
          items={filteredDevItems}
          expanded={isFiltering || isGroupExpanded('dev')}
          collapsed={false}
          onToggle={() => toggleGroup('dev')}
        />
      {/if}

      {#if isFiltering && filteredLibraryItems.length === 0 && filteredMediaItems.length === 0 && filteredLivetvItems.length === 0 && filteredLiveItems.length === 0 && filteredActivityItems.length === 0 && filteredSettingsItems.length === 0 && filteredDevItems.length === 0}
        <div class="no-results">No matches</div>
      {/if}
    </nav>

    <div class="menu-footer">
      <a
        href="/profile"
        class="user-info"
        title={authState.email}
        onclick={() => {
          onClose();
        }}
      >
        <div class="user-avatar">
          {#if authState.avatarUrl}
            <img src={authState.avatarUrl} alt="Avatar" class="avatar-img" />
          {:else}
            <span>{avatarLetter}</span>
          {/if}
        </div>
        <span class="user-email">{authState.email}</span>
      </a>
      <button
        class="logout-btn"
        type="button"
        aria-label="Log out"
        onclick={handleLogout}
        title="Log out"
      >
        <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
          <path
            d="M17 7l-1.41 1.41L18.17 11H8v2h10.17l-2.58 2.58L17 17l5-5-5-5zM4 5h8V3H4c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h8v-2H4V5z"
          ></path>
        </svg>
      </button>
    </div>
  </div>
{/if}

<style>
  .menu-drawer {
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    padding-top: var(--safe-top);
    padding-bottom: var(--safe-bottom);
    padding-left: var(--safe-left);
    width: calc(min(17.5rem, 85vw) + var(--safe-left));
    background: var(--bg);
    border-right: 1px solid var(--ring);
    z-index: 60;
    display: flex;
    flex-direction: column;
    box-shadow: 4px 0 24px rgba(0, 0, 0, 0.3);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(6, 6, 6, 0.5);
    z-index: 59;
    border: none;
    padding: 0;
    cursor: pointer;
  }

  :global([data-theme='light']) .menu-backdrop {
    background: rgba(0, 0, 0, 0.3);
  }

  .menu-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--ring);
  }

  .menu-brand {
    font-family: 'Palatino Linotype', 'Book Antiqua', Palatino, serif;
    font-size: 1.25rem;
    letter-spacing: 0.6px;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0.5rem;
    border-radius: 0.75rem;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
  }

  .menu-nav {
    flex: 1;
    overflow-y: auto;
    padding: 0.75rem;
  }

  /* Home link */
  .home-link {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 4px;
    padding: 10px 12px;
    border-radius: 10px;
    color: var(--ink);
    text-decoration: none;
    font-size: 14px;
    font-weight: 500;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .home-link:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  :global([data-theme='light']) .home-link:hover {
    background: rgba(0, 0, 0, 0.06);
  }

  .home-link.active {
    background: rgba(211, 140, 90, 0.16);
    color: var(--accent);
  }

  :global([data-theme='light']) .home-link.active {
    background: rgba(200, 122, 70, 0.18);
  }

  .home-link svg {
    flex-shrink: 0;
    opacity: 0.8;
  }

  .home-link.active svg {
    opacity: 1;
  }

  /* Filter input */
  .sidebar-filter {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0 8px;
    padding: 6px 10px;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  :global([data-theme='light']) .sidebar-filter {
    background: rgba(0, 0, 0, 0.04);
  }

  .filter-icon {
    flex-shrink: 0;
    color: var(--muted);
  }

  .filter-input {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--ink);
    font-size: 12px;
    outline: none;
    min-width: 0;
  }

  .filter-input::placeholder {
    color: var(--muted);
  }

  .filter-clear {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--muted);
    cursor: pointer;
  }

  .filter-clear:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .no-results {
    text-align: center;
    padding: 16px;
    color: var(--muted);
    font-size: 12px;
  }

  .menu-divider {
    height: 1px;
    background: var(--ring);
    margin: 0.5rem 0.75rem;
  }

  /* User footer */
  .menu-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-top: 1px solid var(--ring);
    flex-shrink: 0;
  }

  .user-info {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    text-decoration: none;
    color: var(--ink);
    border-radius: 8px;
    padding: 4px;
    margin: -4px;
  }

  .user-avatar {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--bg);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    font-weight: 600;
    flex-shrink: 0;
    overflow: hidden;
  }

  .avatar-img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .user-email {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .logout-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
  }

  .logout-btn:hover {
    color: var(--danger);
    background: rgba(239, 68, 68, 0.1);
  }

  /* Format filter chips in Library group header */
  .format-chips {
    display: flex;
    gap: 1px;
    background: var(--ring);
    border-radius: 5px;
    overflow: hidden;
    margin-left: auto;
    margin-right: 4px;
  }

  .fmt-chip {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 18px;
    padding: 0;
    border: none;
    background: var(--card);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .fmt-chip :global(.material-icons-outlined) {
    font-size: 11px;
  }

  .fmt-chip:hover {
    color: var(--ink);
  }

  .fmt-chip.active {
    background: var(--accent-dim);
    color: var(--accent);
  }
</style>
