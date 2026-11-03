// Static registry for pages, commands, and sections
// Extracted from Sidebar.svelte navigation structure

import { getAuthState } from '#lib/stores/auth.svelte.js';
import type { PageItem, CommandItem, SectionItem } from './types';

// SVG paths for icons
export const icons = {
  home: 'M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z',
  books:
    'M6 4h12a2 2 0 0 1 2 2v12h-2V6H6V4zm0 4h10a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2zm0 2v8h10v-8H6z',
  series: 'M3 7h18v2H3V7zm2 4h14v2H5v-2zm-2 4h18v2H3v-2z',
  authors: 'M12 4a3 3 0 1 1 0 6 3 3 0 0 1 0-6zm-6 14a6 6 0 0 1 12 0H6z',
  shelves: 'M4 4h16v2H4V4zm0 4h16v3H4V8zm0 5h16v3H4v-3zm0 5h16v2H4v-2z',
  monitored:
    'M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z',
  ingest: 'M4 6h16v2H4V6zm0 5h16v2H4v-2zm0 5h16v2H4v-2z',
  logs: 'M4 4h16v2H4V4zm0 6h10v2H4v-2zm0 6h16v2H4v-2z',
  proxy: 'M3 3h18v2H3V3zm0 4h12v2H3V7zm0 4h18v2H3v-2zm0 4h12v2H3v-2zm0 4h18v2H3v-2z',
  chatLogs:
    'M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-9 9H9V9h2v2zm4 0h-2V9h2v2z',
  devices:
    'M7 2h10v2H7V2zm-2 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zm0 2v10h14V8H5z',
  users:
    'M16 11c1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3 1.34 3 3 3zm-8 0c1.66 0 3-1.34 3-3S9.66 5 8 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 2 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z',
  chatIdentities:
    'M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-2 12H6v-2h12v2zm0-3H6V9h12v2zm0-3H6V6h12v2z',
  chatMemory:
    'M13 3a9 9 0 0 0-9 9H1l3.89 3.89.07.14L9 12H6a7 7 0 1 1 2.05 4.95l-1.42 1.42A9 9 0 1 0 13 3zm-1 5v5l4.28 2.54.72-1.21-3.5-2.08V8H12z',
  sessions:
    'M17 1H7c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-2-2-2zm0 18H7V5h10v14zm-5-1.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z',
  tokens:
    'M12.65 10C11.83 7.67 9.61 6 7 6c-3.31 0-6 2.69-6 6s2.69 6 6 6c2.61 0 4.83-1.67 5.65-4H17v4h4v-4h2v-4H12.65zM7 14c-1.1 0-2-.9-2-2s.9-2 2-2 2 .9 2 2-.9 2-2 2z',
  general:
    'M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zm8.94 3a8.08 8.08 0 0 0-.66-1.6l2.12-1.65-1.5-2.6-2.5 1a8.32 8.32 0 0 0-1.38-1.02l.38-2.7h-3l-.38 2.7c-.5.14-.97.34-1.42.58l-2-1.65-2.1 2.1 1.65 2a7.65 7.65 0 0 0-.58 1.42l-2.7.38v3l2.7.38c.14.5.34.97.58 1.42l-1.65 2 2.1 2.1 2-1.65c.45.24.92.44 1.42.58l.38 2.7h3l.38-2.7c.5-.14.97-.34 1.42-.58l2 1.65 2.1-2.1-1.65-2c.24-.45.44-.92.58-1.42l2.7-.38v-3l-2.7-.38z',
  obtain:
    'M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM17 13l-5 5-5-5h3V9h4v4h3z',
  streaming:
    'M21 3H3c-1.11 0-2 .89-2 2v12a2 2 0 0 0 2 2h5v2h8v-2h5c1.1 0 1.99-.9 1.99-2L23 5a2 2 0 0 0-2-2zm0 14H3V5h18v12zm-5-6-7 4V7l7 4z',
  cast: 'M1 18v3h3c0-1.66-1.34-3-3-3zm0-4v2c2.76 0 5 2.24 5 5h2c0-3.87-3.13-7-7-7zm0-4v2c4.97 0 9 4.03 9 9h2c0-6.08-4.93-11-11-11zm20-7H3c-1.1 0-2 .9-2 2v3h2V5h18v14h-7v2h7c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z',
  metadata: 'M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zM6 20V4h7v5h5v11H6z',
  integrations:
    'M17 7h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1 0 1.43-.98 2.63-2.31 2.98l1.46 1.46C20.88 15.61 22 13.95 22 12c0-2.76-2.24-5-5-5zm-1 4h-2.19l2 2H16v-2zM2 4.27l3.11 3.11C3.29 8.12 2 9.91 2 12c0 2.76 2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1 0-1.59 1.21-2.9 2.76-3.07L8.73 11H8v2h2.73L13 15.27V17h1.73l4.01 4L20 19.74 3.27 3 2 4.27z',
  notifications:
    'M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z',
  jobs: 'M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V5h14v14zM7 10h2v7H7zm4-3h2v10h-2zm4 6h2v4h-2z',
  backup:
    'M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z',
  theme:
    'M12 3c-4.97 0-9 4.03-9 9s4.03 9 9 9 9-4.03 9-9c0-.46-.04-.92-.1-1.36-.98 1.37-2.58 2.26-4.4 2.26-2.98 0-5.4-2.42-5.4-5.4 0-1.81.89-3.42 2.26-4.4-.44-.06-.9-.1-1.36-.1z',
  search:
    'M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z',
  refresh:
    'M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z',
  movie:
    'M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z',
  tv: 'M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z',
  downloads: 'M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z',
  'media-roots':
    'M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm-6 12H4V8h16v10h-6zm-2-6l-4 4h3v2h2v-2h3l-4-4z',
  people:
    'M16 11c1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3 1.34 3 3 3zm-8 0c1.66 0 3-1.34 3-3S9.66 5 8 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 2 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z',
  discover:
    'M12 10.9c-.61 0-1.1.49-1.1 1.1s.49 1.1 1.1 1.1c.61 0 1.1-.49 1.1-1.1s-.49-1.1-1.1-1.1zM12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm2.19 12.19L6 18l3.81-8.19L18 6l-3.81 8.19z',
  sports:
    'M19 5h-2V3H7v2H5c-1.1 0-2 .9-2 2v1c0 2.55 1.92 4.63 4.39 4.94.63 1.5 1.98 2.63 3.61 2.96V19H7v2h10v-2h-4v-3.1c1.63-.33 2.98-1.46 3.61-2.96C19.08 12.63 21 10.55 21 8V7c0-1.1-.9-2-2-2zM5 8V7h2v3.82C5.84 10.4 5 9.3 5 8zm14 0c0 1.3-.84 2.4-2 2.82V7h2v1z',
  iptv: 'M17 10.5V7c0-.55-.45-1-1-1H4c-.55 0-1 .45-1 1v10c0 .55.45 1 1 1h12c.55 0 1-.45 1-1v-3.5l4 4v-11l-4 4zM14 17H5V7h9v10z',
  livetv:
    'M21 6h-7.59l3.29-3.29L16 2l-4 4-4-4-.71.71L10.59 6H3c-1.1 0-2 .89-2 2v12c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2V8c0-1.11-.9-2-2-2zm0 14H3V8h18v12zM9 10v8l7-4-7-4z',
  migration: 'M6.99 11L3 15l3.99 4v-3H14v-2H6.99v-3zM21 9l-3.99-4v3H10v2h7.01v3L21 9z',
  kobo: 'M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 18H6V4h2v8l2.5-1.5L13 12V4h5v16z',
  processing:
    'M17 16l-4-4V8.82C14.16 8.4 15 7.3 15 6c0-1.66-1.34-3-3-3S9 4.34 9 6c0 1.3.84 2.4 2 2.82V12l-4 4H3v5h5v-3.05l4-4.2 4 4.2V21h5v-5h-4z',
  audio:
    'M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z',
  voice:
    'M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3zm-1-9c0-.55.45-1 1-1s1 .45 1 1v6c0 .55-.45 1-1 1s-1-.45-1-1V5zm6 6c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z',
  subtitles:
    'M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zM4 18V6h16v12H4zm2-6h2v2H6v-2zm0 3h8v2H6v-2zm10 0h2v2h-2v-2zm-6-3h8v2h-8v-2z',
  profile: 'M12 4a3 3 0 1 1 0 6 3 3 0 0 1 0-6zm-6 14a6 6 0 0 1 12 0H6z'
};

export const pages: PageItem[] = [
  // Home
  {
    id: 'home',
    label: 'Home',
    path: '/',
    aliases: ['home', 'discover', 'find', 'search', 'add', 'new'],
    icon: icons.home
  },
  {
    id: 'dashboard',
    label: 'Dashboard',
    path: '/dashboard',
    aliases: ['dashboard', 'activity', 'overview'],
    icon: 'M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z'
  },

  // Library
  {
    id: 'library',
    label: 'Books',
    path: '/library',
    aliases: ['library', 'collection'],
    icon: icons.books
  },
  { id: 'series', label: 'Series', path: '/series', aliases: [], icon: icons.series },
  { id: 'authors', label: 'Authors', path: '/authors', aliases: [], icon: icons.authors },
  {
    id: 'shelves',
    label: 'Shelves',
    path: '/shelves',
    aliases: ['collections', 'tags'],
    icon: icons.shelves
  },
  // Media
  {
    id: 'movies',
    label: 'Movies',
    path: '/movies',
    aliases: ['movie', 'film', 'cinema', 'video'],
    icon: icons.movie
  },
  {
    id: 'tv-shows',
    label: 'TV Shows',
    path: '/tv',
    aliases: ['television', 'show', 'episode', 'series'],
    icon: icons.tv
  },
  {
    id: 'sports',
    label: 'Sports',
    path: '/sports',
    aliases: ['league', 'motorsport', 'motogp', 'race', 'racing', 'formula'],
    icon: icons.sports
  },
  {
    id: 'livetv',
    label: 'Live TV Guide',
    path: '/livetv',
    aliases: ['live', 'tv', 'tuner', 'hdhomerun', 'antenna', 'epg', 'guide', 'broadcast'],
    icon: icons.livetv
  },
  {
    id: 'livetv-channels',
    label: 'Live TV Channels',
    path: '/livetv/channels',
    aliases: ['channels', 'lineup', 'scan', 'curate', 'mux', 'lcn', 'tuner'],
    icon: icons.livetv
  },
  {
    id: 'livetv-recordings',
    label: 'Live TV Recordings',
    path: '/livetv/recordings',
    aliases: ['dvr', 'record', 'recording', 'tuner'],
    icon: icons.livetv
  },
  {
    id: 'livetv-scheduled',
    label: 'Live TV Scheduled',
    path: '/livetv/scheduled',
    aliases: ['scheduled', 'series rule', 'upcoming', 'dvr'],
    icon: icons.livetv
  },
  {
    id: 'livetv-on-now',
    label: 'Live TV On Now',
    path: '/livetv/on-now',
    aliases: ['on now', 'watch', 'suggestions', 'recommended', 'live'],
    icon: icons.livetv
  },
  {
    id: 'livetv-series-rules',
    label: 'Live TV Series Rules',
    path: '/livetv/series-rules',
    aliases: ['series rule', 'rules', 'season pass', 'dvr', 'record'],
    icon: icons.livetv
  },
  {
    id: 'iptv',
    label: 'IPTV Home',
    path: '/iptv',
    aliases: ['iptv', 'playlist', 'm3u', 'xtream', 'home'],
    icon: icons.iptv
  },
  {
    id: 'iptv-channels',
    label: 'IPTV Channels',
    path: '/iptv/channels',
    aliases: ['channels', 'stream', 'iptv'],
    icon: icons.iptv
  },
  {
    id: 'iptv-guide',
    label: 'IPTV Guide',
    path: '/iptv/guide',
    aliases: ['epg', 'program', 'schedule', 'guide', 'iptv'],
    icon: icons.iptv
  },
  {
    id: 'iptv-recordings',
    label: 'IPTV Recordings',
    path: '/iptv/recordings',
    aliases: ['recording', 'iptv'],
    icon: icons.iptv
  },
  {
    id: 'discover-settings',
    label: 'Discover Settings',
    path: '/settings/discover',
    aliases: ['streaming services', 'netflix', 'region'],
    icon: icons.discover
  },
  {
    id: 'playback-reports',
    label: 'Playback Reports',
    path: '/settings/diagnostics',
    aliases: ['errors', 'failures', 'diagnostics', 'crash', 'client reports'],
    icon: icons.jobs
  },
  {
    id: 'livetv-settings',
    label: 'Live TV Settings',
    path: '/settings/livetv',
    aliases: ['tuner', 'hdhomerun', 'dvr', 'ad detection', 'buffer', 'timeshift', 'rescan'],
    icon: icons.livetv
  },
  {
    id: 'iptv-settings',
    label: 'IPTV Settings',
    path: '/settings/iptv',
    aliases: ['provider', 'channels', 'unmatched'],
    icon: icons.iptv
  },
  {
    id: 'people',
    label: 'People',
    path: '/people',
    aliases: ['person', 'actor', 'director', 'cast', 'crew'],
    icon: icons.people
  },
  {
    id: 'calendar',
    label: 'Calendar',
    path: '/calendar',
    aliases: ['upcoming', 'releases', 'schedule'],
    icon: 'M19 3h-1V1h-2v2H8V1H6v2H5c-1.11 0-1.99.9-1.99 2L3 19c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V8h14v11zM9 10H7v2h2v-2zm4 0h-2v2h2v-2zm4 0h-2v2h2v-2z'
  },

  // Obtain
  {
    id: 'wanted',
    label: 'Wanted Books',
    path: '/library?ownership=wanted',
    aliases: ['track', 'tracking', 'follow', 'wanted', 'queue', 'download'],
    icon: icons.monitored
  },
  {
    id: 'ingest',
    label: 'Ingest Queue',
    path: '/ingest-queue',
    aliases: ['import', 'upload', 'add'],
    icon: icons.ingest
  },

  // Activity
  {
    id: 'downloads',
    label: 'Downloads',
    path: '/downloads',
    aliases: ['download', 'active', 'transfer'],
    icon: icons.downloads
  },
  {
    id: 'activity',
    label: 'Activity Logs',
    path: '/activity',
    aliases: ['logs', 'history'],
    icon: icons.logs
  },
  {
    id: 'proxy',
    label: 'Proxy Logs',
    path: '/proxy-logs',
    aliases: ['kobo', 'sync'],
    icon: icons.proxy
  },
  {
    id: 'chat-logs',
    label: 'Chat Turn Logs',
    path: '/chat-logs',
    aliases: ['chat', 'bot', 'turn', 'llm', 'assistant', 'debug'],
    icon: icons.chatLogs
  },

  // Settings
  {
    id: 'devices',
    label: 'Devices',
    path: '/settings/devices',
    aliases: ['kobo', 'reader', 'ereader'],
    icon: icons.devices
  },
  {
    id: 'media-roots',
    label: 'Media Roots',
    path: '/settings/media-roots',
    aliases: ['root', 'folder', 'directory', 'scan', 'media', 'library'],
    icon: icons['media-roots']
  },
  { id: 'users', label: 'Users', path: '/settings/users', aliases: [], icon: icons.users },
  {
    id: 'chat-identities',
    label: 'Chat Identities',
    path: '/settings/chat-identities',
    aliases: ['chat', 'bot', 'google', 'pairing', 'identity'],
    icon: icons.chatIdentities
  },
  {
    id: 'chat-memory',
    label: 'Chat Memory',
    path: '/settings/chat-memory',
    aliases: ['memory', 'facts', 'profile', 'remember', 'forget', 'privacy', 'assistant'],
    icon: icons.chatMemory
  },
  {
    id: 'sessions',
    label: 'Sessions',
    path: '/settings/sessions',
    aliases: ['device', 'login', 'revoke', 'signed in', 'logout', 'linked'],
    icon: icons.sessions
  },
  {
    id: 'watch-history',
    label: 'Watch History',
    path: '/settings/watch-history',
    aliases: ['history', 'watched', 'recommendations', 'for you', 'trakt', 'hide', 'remove'],
    icon: icons.chatMemory
  },
  {
    id: 'stremio-history',
    label: 'Stremio History',
    path: '/settings/stremio-history',
    aliases: ['stremio', 'import', 'watched', 'migrate', 'continue watching'],
    icon: icons.chatMemory
  },
  {
    id: 'tokens',
    label: 'API Tokens',
    path: '/settings/tokens',
    aliases: ['api', 'key', 'auth'],
    icon: icons.tokens
  },
  {
    id: 'general',
    label: 'General Settings',
    path: '/settings/general',
    aliases: ['config', 'configuration', 'system', 'watcher', 'cache', 'pagination'],
    icon: icons.general
  },
  {
    id: 'audio',
    label: 'Audio & Speech Settings',
    path: '/settings/audio',
    aliases: ['tts', 'text-to-speech', 'speech', 'transcription', 'whisper', 'stt'],
    icon: icons.audio
  },
  {
    id: 'voice',
    label: 'Voice Presets',
    path: '/settings/voice',
    aliases: ['tts', 'voice', 'preset', 'clone', 'emotion', 'speech'],
    icon: icons.voice
  },
  {
    id: 'obtain',
    label: 'Obtain Settings',
    path: '/settings/obtain',
    aliases: ['download', 'kobload'],
    icon: icons.obtain
  },
  {
    id: 'streaming',
    label: 'Streaming Settings',
    path: '/settings/streaming',
    aliases: ['debrid', 'stremio', 'real-debrid', 'torbox', 'cache'],
    icon: icons.streaming
  },
  {
    id: 'cast',
    label: 'Cast Settings',
    path: '/settings/cast',
    aliases: ['chromecast', 'google tv', 'casting', 'receiver'],
    icon: icons.cast
  },
  {
    id: 'metadata',
    label: 'Metadata Settings',
    path: '/settings/metadata',
    aliases: ['info', 'data'],
    icon: icons.metadata
  },
  {
    id: 'subtitles',
    label: 'Subtitle Settings',
    path: '/settings/subtitles',
    aliases: ['subtitle', 'opensubtitles', 'captions', 'srt', 'vtt'],
    icon: icons.subtitles
  },
  {
    id: 'search-settings',
    label: 'Search & AI Settings',
    path: '/settings/search',
    aliases: ['meili', 'meilisearch', 'index', 'indexer', 'koblem', 'llm', 'chat'],
    icon: icons.search
  },
  {
    id: 'integrations',
    label: 'Integrations',
    path: '/settings/integrations',
    aliases: ['connect', 'external'],
    icon: icons.integrations
  },
  {
    id: 'notification-inbox',
    label: 'Notification Inbox',
    path: '/notifications',
    aliases: ['inbox', 'alerts', 'bell'],
    icon: icons.notifications
  },
  {
    id: 'notification-settings',
    label: 'Notification Settings',
    path: '/settings/notifications',
    aliases: ['notify', 'push', 'pushover', 'plex', 'alert'],
    icon: icons.notifications
  },
  {
    id: 'jobs',
    label: 'Jobs',
    path: '/jobs',
    aliases: ['tasks', 'background', 'scheduled', 'queue', 'history'],
    icon: icons.jobs
  },
  {
    id: 'job-concurrency',
    label: 'Job Concurrency',
    path: '/settings/jobs',
    aliases: ['parallelism', 'workers', 'threads'],
    icon: icons.jobs
  },
  {
    id: 'backup',
    label: 'Backup',
    path: '/settings/backup',
    aliases: ['export', 'restore', 'save'],
    icon: icons.backup
  },
  {
    id: 'migration',
    label: 'Migration',
    path: '/settings/migration',
    aliases: ['sonarr', 'radarr', 'import', 'migrate'],
    icon: icons.migration
  },
  {
    id: 'kobo-proxy',
    label: 'Kobo Proxy Settings',
    path: '/settings/kobo',
    aliases: ['kobo', 'proxy', 'sync', 'entitlement'],
    icon: icons.kobo
  },
  {
    id: 'processing',
    label: 'Processing Settings',
    path: '/settings/processing',
    aliases: ['enrichment', 'thumbnails', 'skip detection', 'intro', 'credits'],
    icon: icons.processing
  },
  {
    id: 'profile',
    label: 'Profile',
    path: '/profile',
    aliases: ['account', 'avatar', 'password', 'user'],
    icon: icons.profile
  }
];

export const commands: CommandItem[] = [
  {
    id: 'toggle-theme',
    label: 'Toggle theme',
    action: 'toggleTheme',
    aliases: ['dark', 'light', 'mode'],
    icon: icons.theme
  },
  {
    id: 'refresh-library',
    label: 'Refresh library',
    action: 'refreshLibrary',
    aliases: ['reload', 'sync'],
    icon: icons.refresh
  },
  {
    id: 'reindex-search',
    label: 'Rebuild search index',
    action: 'reindexSearch',
    aliases: ['rebuild', 'index'],
    icon: icons.search,
    minRole: 'admin'
  },
  {
    id: 'reindex-transcripts',
    label: 'Rebuild transcript index',
    action: 'reindexTranscripts',
    aliases: ['rebuild', 'transcript', 'audiobook', 'ebook', 'text'],
    icon: icons.search,
    minRole: 'admin'
  },
  {
    id: 'clear-metadata-cache',
    label: 'Clear metadata cache',
    action: 'clearMetadataCache',
    aliases: ['cache', 'metadata', 'reset'],
    icon: icons.metadata,
    confirmMessage: 'Clear all cached metadata? This will slow down the next metadata lookups.',
    minRole: 'admin'
  },
  {
    id: 'library-cleanup',
    label: 'Clean up library',
    action: 'libraryCleanup',
    aliases: ['cleanup', 'orphan', 'remove', 'unowned', 'sweep'],
    icon: icons.books,
    confirmMessage:
      'Delete all orphaned entities (books, movies, episodes without files or wants, and their orphaned parents)? This cannot be undone.',
    minRole: 'admin'
  },
  {
    id: 'write-metadata-all',
    label: 'Write metadata to files',
    action: 'writeMetadataAll',
    aliases: ['embed', 'write', 'metadata'],
    icon: icons.metadata,
    confirmMessage: 'Write metadata to all book files? This may take a while.',
    minRole: 'admin'
  },
  {
    id: 'sidecar-rewrite',
    label: 'Rewrite all sidecars',
    action: 'sidecarRewrite',
    aliases: ['sidecar', 'nfo', 'rewrite', 'regenerate'],
    icon: icons.metadata,
    confirmMessage:
      'Regenerate all sidecar files? TV shows will be re-enriched first. This may take a while.',
    minRole: 'admin'
  },
  {
    id: 'clear-proxy-logs',
    label: 'Clear proxy logs',
    action: 'clearProxyLogs',
    aliases: ['proxy', 'logs', 'clean'],
    icon: icons.proxy,
    minRole: 'admin'
  },
  {
    id: 'clear-completed-jobs',
    label: 'Clear job history',
    action: 'clearCompletedJobs',
    aliases: ['jobs', 'history', 'clean'],
    icon: icons.jobs,
    minRole: 'admin'
  },
  {
    id: 'clear-recent-searches',
    label: 'Clear recent searches',
    action: 'clearRecentSearches',
    aliases: ['recents', 'frecency', 'history'],
    icon: icons.search
  }
];

export const sections: SectionItem[] = [
  // Library sections
  {
    id: 'library-synced',
    label: 'Synced books',
    path: '/library?filter=synced',
    parent: 'library'
  },
  {
    id: 'library-unsynced',
    label: 'Unsynced books',
    path: '/library?filter=pending',
    parent: 'library'
  }
];

// Search helpers

function commandAllowed(cmd: CommandItem): boolean {
  if (!cmd.minRole) return true;
  const auth = getAuthState();
  return cmd.minRole === 'admin' ? auth.isAdmin : auth.canManageContent;
}

export function searchPages(query: string): PageItem[] {
  if (!query) return pages.slice(0, 5);
  const q = query.toLowerCase();
  return pages.filter(
    (page) =>
      page.label.toLowerCase().includes(q) ||
      page.id.toLowerCase().includes(q) ||
      page.aliases.some((alias) => alias.includes(q))
  );
}

export function searchCommands(query: string): CommandItem[] {
  const allowed = commands.filter(commandAllowed);
  if (!query) return allowed;
  const q = query.toLowerCase();
  return allowed.filter(
    (cmd) =>
      cmd.label.toLowerCase().includes(q) ||
      cmd.id.toLowerCase().includes(q) ||
      cmd.aliases.some((alias) => alias.includes(q))
  );
}

export function searchSections(query: string): SectionItem[] {
  if (!query) return [];
  const q = query.toLowerCase();
  return sections.filter(
    (section) => section.label.toLowerCase().includes(q) || section.id.toLowerCase().includes(q)
  );
}
