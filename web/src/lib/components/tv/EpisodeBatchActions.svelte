<script lang="ts">
  import type { BulkEpisodeAction } from '$lib/types';

  interface Props {
    selectedCount: number;
    totalCount: number;
    loading?: boolean;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onApply: (action: BulkEpisodeAction) => void;
    noun?: string;
  }

  let {
    selectedCount,
    totalCount,
    loading = false,
    onSelectAll,
    onClearSelection,
    onApply,
    noun = 'episode'
  }: Props = $props();

  const disabled = $derived(loading || selectedCount === 0);
  const allSelected = $derived(totalCount > 0 && selectedCount === totalCount);
</script>

<div class="batch-actions">
  <div class="selection-info">
    <span class="count">{selectedCount} of {totalCount} selected</span>
    <span class="hint">shift-click for a range</span>
    <button class="btn-link" type="button" onclick={allSelected ? onClearSelection : onSelectAll}>
      {allSelected ? 'Clear' : 'Select all'}
    </button>
  </div>

  <div class="batch-controls">
    <button class="btn-action" type="button" {disabled} onclick={() => onApply('want')}>
      <span class="material-icons-outlined">add_circle_outline</span>
      Want
    </button>
    <button class="btn-action" type="button" {disabled} onclick={() => onApply('unwant')}>
      <span class="material-icons-outlined">remove_circle_outline</span>
      Unwant
    </button>
    <button class="btn-action" type="button" {disabled} onclick={() => onApply('mark_watched')}>
      <span class="material-icons-outlined">visibility</span>
      Watched
    </button>
    <button class="btn-action" type="button" {disabled} onclick={() => onApply('mark_unwatched')}>
      <span class="material-icons-outlined">visibility_off</span>
      Unwatched
    </button>

    <span class="divider"></span>

    <button
      class="btn-action danger"
      type="button"
      {disabled}
      title="Delete the media, keep the {noun}s so they can be re-acquired"
      onclick={() => onApply('delete_files')}
    >
      <span class="material-icons-outlined">delete_outline</span>
      Delete files
    </button>
    <button
      class="btn-action danger"
      type="button"
      {disabled}
      title="Delete the media and remove the {noun}s entirely"
      onclick={() => onApply('delete_episodes')}
    >
      <span class="material-icons-outlined">playlist_remove</span>
      Delete {noun}s
    </button>
  </div>
</div>

<style>
  .batch-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 10px 16px;
    background: var(--card);
    border: 1px solid var(--accent-dim);
    border-radius: 10px;
    margin-bottom: 16px;
    position: sticky;
    top: 8px;
    z-index: 5;
  }

  .selection-info {
    display: flex;
    align-items: center;
    gap: 12px;
    white-space: nowrap;
  }

  .count {
    font-size: 13px;
    font-weight: 600;
  }

  .hint {
    font-size: 11px;
    color: var(--muted);
  }

  .btn-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    text-decoration: underline;
    padding: 0;
  }

  .btn-link:hover {
    opacity: 0.8;
  }

  .batch-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .divider {
    width: 1px;
    height: 20px;
    background: var(--ring);
    margin: 0 4px;
  }

  .btn-action {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg);
    color: var(--ink);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .btn-action .material-icons-outlined {
    font-size: 15px;
  }

  .btn-action:hover:not(:disabled) {
    background: var(--bg-2);
    border-color: var(--accent-dim);
  }

  .btn-action.danger {
    color: var(--danger, #e05260);
  }

  .btn-action.danger:hover:not(:disabled) {
    border-color: var(--danger, #e05260);
  }

  .btn-action:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  @media (hover: none) {
    .hint {
      display: none;
    }
  }

  @media (max-width: 768px) {
    .batch-actions {
      flex-direction: column;
      align-items: stretch;
      gap: 10px;
    }

    .batch-controls {
      justify-content: flex-start;
    }

    .divider {
      display: none;
    }
  }
</style>
