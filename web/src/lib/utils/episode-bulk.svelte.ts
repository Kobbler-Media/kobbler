import { SvelteSet } from 'svelte/reactivity';
import type { BulkEpisodeAction, BulkEpisodeResult } from '#lib/types.js';
import * as api from '#lib/api/client.js';

const ACTION_LABELS: Record<BulkEpisodeAction, string> = {
  delete_files: 'Delete files',
  delete_episodes: 'Delete episodes',
  want: 'Want',
  unwant: 'Unwant',
  mark_watched: 'Mark watched',
  mark_unwatched: 'Mark unwatched'
};

/** Mirrors BULK_EPISODE_LIMIT in api/src/routes/admin/tv.rs; larger selections go in batches. */
const BATCH_SIZE = 2000;

async function applyInBatches(
  episodeIds: string[],
  action: BulkEpisodeAction
): Promise<BulkEpisodeResult> {
  const total: BulkEpisodeResult = {
    episodes_affected: 0,
    files_deleted: 0,
    auto_want_disabled: false
  };
  for (let i = 0; i < episodeIds.length; i += BATCH_SIZE) {
    const result = await api.bulkEpisodeAction(episodeIds.slice(i, i + BATCH_SIZE), action);
    total.episodes_affected += result.episodes_affected;
    total.files_deleted += result.files_deleted;
    total.auto_want_disabled ||= result.auto_want_disabled;
  }
  return total;
}

function plural(noun: string, n: number): string {
  return `${n} ${noun}${n === 1 ? '' : 's'}`;
}

/**
 * Confirmation text for the destructive actions. The episode-delete warning
 * names the recreation trap: a monitored show re-adds any episode its metadata
 * provider still lists, so deleting extras only sticks once the show is matched
 * to the right series.
 */
function confirmText(action: BulkEpisodeAction, count: number, noun: string): string | null {
  const n = plural(noun, count);
  switch (action) {
    case 'delete_files':
      return `Delete the media for ${n}? The files are removed from disk. The ${noun}s stay, so they can be downloaded again.`;
    case 'delete_episodes':
      return `Delete ${n} entirely, along with their files on disk? If this is monitored, any ${noun} its metadata provider still lists will come back on the next refresh — fix the match first.`;
    default:
      return null;
  }
}

function resultMessage(action: BulkEpisodeAction, result: BulkEpisodeResult, noun: string): string {
  const eps = (n: number) => plural(noun, n);
  switch (action) {
    case 'delete_files': {
      const n = result.files_deleted;
      return n === 0 ? 'No files to delete' : `Deleted ${plural('file', n)}`;
    }
    case 'delete_episodes':
      return `Deleted ${eps(result.episodes_affected)} and ${plural('file', result.files_deleted)}`;
    case 'want':
      return result.episodes_affected === 0
        ? 'Nothing to want — all already in the library'
        : `Wanted ${eps(result.episodes_affected)}`;
    case 'unwant': {
      const base =
        result.episodes_affected === 0
          ? 'No outstanding wants in the selection'
          : `Unwanted ${eps(result.episodes_affected)}`;
      return result.auto_want_disabled
        ? `${base} — auto-want turned off so they stay unwanted`
        : base;
    }
    case 'mark_watched':
      return `Marked ${eps(result.episodes_affected)} as watched`;
    case 'mark_unwatched':
      return `Marked ${eps(result.episodes_affected)} as unwatched`;
  }
}

interface BulkSelectionOptions {
  /** Awaited after a successful apply — the caller reloads whatever it renders from. */
  onDone: () => Promise<void> | void;
  notify: (message: string) => void;
  /**
   * Turns the selected ids into episode ids at apply time. The sports league page
   * selects seasons, whose events are not loaded until an action needs them.
   */
  resolve?: (ids: string[]) => Promise<string[]>;
  noun?: string;
}

/**
 * Selection state plus the apply/confirm cycle for bulk episode edits, shared by
 * the TV and sports show/season pages so all of them behave identically.
 */
export class EpisodeBulkSelection {
  active = $state(false);
  readonly ids = new SvelteSet<string>();
  busy = $state(false);

  #options: BulkSelectionOptions;
  /** Last row clicked, the fixed end of a shift-click range. */
  #anchor: string | null = null;

  constructor(options: BulkSelectionOptions) {
    this.#options = options;
  }

  get count(): number {
    return this.ids.size;
  }

  has(id: string): boolean {
    return this.ids.has(id);
  }

  toggleMode(): void {
    this.active = !this.active;
    if (!this.active) this.clear();
  }

  /**
   * Toggle one row, or — with shift held — every row between the last click and
   * this one. `within` is the ordered list the row belongs to, so a range never
   * spans two seasons. The range takes its direction from the clicked row:
   * shift-clicking an unselected row selects the span, shift-clicking a selected
   * one clears it.
   */
  toggle(id: string, within: { id: string }[] = [], range = false): void {
    const anchor = this.#anchor;
    if (range && anchor !== null && anchor !== id) {
      const from = within.findIndex((e) => e.id === anchor);
      const to = within.findIndex((e) => e.id === id);
      if (from !== -1 && to !== -1) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        const select = !this.ids.has(id);
        for (let i = lo; i <= hi; i++) {
          if (select) {
            this.ids.add(within[i].id);
          } else {
            this.ids.delete(within[i].id);
          }
        }
        this.#anchor = id;
        return;
      }
    }

    if (this.ids.has(id)) {
      this.ids.delete(id);
    } else {
      this.ids.add(id);
    }
    this.#anchor = id;
  }

  /** Add every row in a group without disturbing selections elsewhere. */
  selectMany(rows: { id: string }[]): void {
    for (const r of rows) this.ids.add(r.id);
  }

  deselectMany(rows: { id: string }[]): void {
    for (const r of rows) this.ids.delete(r.id);
  }

  allSelected(rows: { id: string }[]): boolean {
    return rows.length > 0 && rows.every((r) => this.ids.has(r.id));
  }

  clear(): void {
    this.ids.clear();
    this.#anchor = null;
  }

  async apply(action: BulkEpisodeAction): Promise<void> {
    if (this.ids.size === 0 || this.busy) return;

    const { onDone, notify, resolve, noun = 'episode' } = this.#options;
    this.busy = true;
    try {
      const selected = resolve ? await resolve([...this.ids]) : [...this.ids];
      if (selected.length === 0) {
        notify(`No ${noun}s in the selection`);
        return;
      }

      const prompt = confirmText(action, selected.length, noun);
      if (prompt && !confirm(prompt)) return;

      const result = await applyInBatches(selected, action);
      notify(resultMessage(action, result, noun));
      this.clear();
      await onDone();
    } catch (e) {
      notify(e instanceof Error ? e.message : `Failed to ${ACTION_LABELS[action].toLowerCase()}`);
    } finally {
      this.busy = false;
    }
  }
}
