export interface ChipDef {
  icon: string;
  label: string;
  color: string;
}

export function mediaTypeChip(mediaType: string): ChipDef {
  switch (mediaType) {
    case 'book':
      return { icon: 'menu_book', label: 'Book', color: 'var(--monitored)' };
    case 'movie':
      return { icon: 'movie', label: 'Movie', color: 'var(--warning)' };
    case 'tv':
      return { icon: 'live_tv', label: 'TV Show', color: 'var(--success)' };
    case 'sport':
      return { icon: 'sports_soccer', label: 'Sport', color: 'var(--accent)' };
    case 'audiobook':
      return { icon: 'headphones', label: 'Audiobook', color: 'var(--monitored)' };
    case 'episode':
      return { icon: 'ondemand_video', label: 'Episode', color: 'var(--success)' };
    case 'event':
      return { icon: 'sports', label: 'Sport Event', color: 'var(--accent)' };
    default:
      return { icon: 'category', label: mediaType, color: 'var(--muted)' };
  }
}

export function statusChip(statusClass: string): ChipDef {
  switch (statusClass) {
    case 'pending':
      return { icon: 'hourglass_empty', label: 'Pending', color: 'var(--accent)' };
    case 'searching':
      return { icon: 'search', label: 'Searching', color: 'var(--monitored)' };
    case 'downloading':
      return { icon: 'download', label: 'Downloading', color: 'var(--success)' };
    case 'watching':
      return { icon: 'visibility', label: 'Watching', color: 'var(--monitored)' };
    case 'failed':
      return { icon: 'error_outline', label: 'Failed', color: 'var(--danger)' };
    case 'wanted':
      return { icon: 'bookmark_add', label: 'Wanted', color: 'var(--wanted)' };
    case 'skipped':
      return { icon: 'skip_next', label: 'Skipped', color: 'var(--skipped)' };
    case 'synced':
      return { icon: 'check_circle', label: 'Synced', color: 'var(--success)' };
    case 'owned':
      return { icon: 'check_circle', label: 'Owned', color: 'var(--success)' };
    default:
      return { icon: 'help_outline', label: statusClass, color: 'var(--muted)' };
  }
}

export function wantedPanelChip(status: string): ChipDef {
  switch (status) {
    case 'held':
      return { icon: 'back_hand', label: 'Held', color: 'var(--wanted)' };
    case 'searching':
      return { icon: 'visibility', label: 'Monitoring', color: 'var(--monitored)' };
    case 'downloading':
      return { icon: 'download', label: 'Downloading', color: 'var(--success)' };
    case 'upgrading':
      return { icon: 'upgrade', label: 'Upgrading', color: 'var(--monitored)' };
    case 'satisfied':
      return { icon: 'check_circle', label: 'Complete', color: 'var(--success)' };
    case 'paused':
      return { icon: 'pause_circle', label: 'Paused', color: 'var(--skipped)' };
    default:
      return { icon: 'help_outline', label: status, color: 'var(--muted)' };
  }
}

export function resultMetaChip(kind: string): ChipDef | null {
  switch (kind) {
    case 'monitored':
      return { icon: 'visibility', label: 'Monitored', color: 'var(--monitored)' };
    case 'in-library':
      return { icon: 'library_books', label: 'In Library', color: 'var(--success)' };
    case 'wanted':
      return { icon: 'bookmark_add', label: 'Wanted', color: 'var(--wanted)' };
    default:
      return null;
  }
}

export function resultTypeChip(type: string): ChipDef {
  switch (type) {
    case 'book':
      return { icon: 'menu_book', label: 'Book', color: 'var(--monitored)' };
    case 'author':
      return { icon: 'person', label: 'Author', color: '#9866b2' };
    case 'series':
      return { icon: 'collections_bookmark', label: 'Series', color: '#48baa3' };
    case 'page':
      return { icon: 'description', label: 'Page', color: 'var(--accent)' };
    case 'section':
      return { icon: 'segment', label: 'Section', color: 'var(--accent)' };
    case 'command':
      return { icon: 'terminal', label: 'Command', color: 'var(--wanted)' };
    case 'movie':
      return { icon: 'movie', label: 'Movie', color: 'var(--warning)' };
    case 'tv_show':
      return { icon: 'live_tv', label: 'TV Show', color: 'var(--success)' };
    case 'tv_season':
      return { icon: 'theaters', label: 'Season', color: 'var(--success)' };
    case 'tv_episode':
      return { icon: 'smart_display', label: 'Episode', color: 'var(--success)' };
    case 'ingest':
      return { icon: 'inbox', label: 'Ingest', color: 'var(--accent)' };
    case 'external_search':
      return { icon: 'search', label: 'Search', color: 'var(--accent)' };
    case 'transcript':
      return { icon: 'text_snippet', label: 'Transcript', color: '#7c9cbf' };
    default:
      return { icon: 'help_outline', label: type, color: 'var(--muted)' };
  }
}

export function movieStatusChip(statusClass: string): ChipDef {
  switch (statusClass) {
    case 'pending':
      return { icon: 'hourglass_empty', label: 'Pending', color: 'var(--accent)' };
    case 'wanted':
      return { icon: 'bookmark_add', label: 'Wanted', color: 'var(--wanted)' };
    case 'monitored':
      return { icon: 'visibility', label: 'Monitored', color: 'var(--monitored)' };
    case 'ended':
      return { icon: 'event_available', label: 'Ended', color: 'var(--skipped)' };
    default:
      return { icon: 'help_outline', label: statusClass, color: 'var(--muted)' };
  }
}

export function tvStatusChip(statusClass: string): ChipDef {
  switch (statusClass) {
    case 'monitored':
      return { icon: 'visibility', label: 'Monitored', color: 'var(--monitored)' };
    case 'ended':
      return { icon: 'event_available', label: 'Ended', color: 'var(--skipped)' };
    default:
      return { icon: 'help_outline', label: statusClass, color: 'var(--muted)' };
  }
}

export function ingestStatusChip(statusClass: string): ChipDef {
  switch (statusClass) {
    case 'review':
      return { icon: 'rate_review', label: 'Review', color: 'var(--wanted)' };
    case 'matched':
      return { icon: 'check', label: 'Matched', color: 'var(--success)' };
    case 'extracting':
      return { icon: 'settings', label: 'Extracting', color: 'var(--accent)' };
    case 'converting':
      return { icon: 'sync', label: 'Converting', color: 'var(--accent)' };
    case 'error':
      return { icon: 'error_outline', label: 'Error', color: 'var(--danger)' };
    case 'rejected':
      return { icon: 'block', label: 'Rejected', color: 'var(--skipped)' };
    default:
      return { icon: 'hourglass_empty', label: statusClass || 'Pending', color: 'var(--muted)' };
  }
}

export function mediaRootStatusChip(status: string): ChipDef {
  switch (status) {
    case 'scanning':
      return { icon: 'radar', label: 'Scanning', color: 'var(--accent)' };
    case 'error':
      return { icon: 'error_outline', label: 'Error', color: 'var(--danger)' };
    case 'unavailable':
      return { icon: 'cloud_off', label: 'Unavailable', color: 'var(--warning)' };
    case 'disabled':
      return { icon: 'block', label: 'Disabled', color: 'var(--skipped)' };
    default:
      return { icon: 'help_outline', label: status, color: 'var(--muted)' };
  }
}
