<script lang="ts">
  interface Props {
    noun: string;
    allWatched: boolean;
    anyWanted: boolean;
    onToggleWatched?: (watched: boolean) => void;
    onWantMissing: () => void;
    onUnwantAll: () => void;
  }

  let { noun, allWatched, anyWanted, onToggleWatched, onWantMissing, onUnwantAll }: Props =
    $props();

  // Rows these sit in are links or expand toggles; a click here must not reach them.
  function run(e: MouseEvent, action: () => void) {
    e.preventDefault();
    e.stopPropagation();
    action();
  }
</script>

<div class="season-actions">
  {#if onToggleWatched}
    <button
      class="season-action icon-only"
      class:watched={allWatched}
      type="button"
      title={allWatched ? 'Mark season unwatched' : 'Mark season watched'}
      onclick={(e) => run(e, () => onToggleWatched(!allWatched))}
    >
      <span class="material-icons-outlined">{allWatched ? 'visibility_off' : 'visibility'}</span>
    </button>
  {/if}
  <button
    class="season-action"
    type="button"
    title="Want missing {noun}s in this season"
    onclick={(e) => run(e, onWantMissing)}
  >
    <span class="material-icons-outlined">playlist_add</span>
    <span class="label">Want Missing</span>
  </button>
  {#if anyWanted}
    <button
      class="season-action"
      type="button"
      title="Unwant every outstanding {noun} in this season"
      onclick={(e) => run(e, onUnwantAll)}
    >
      <span class="material-icons-outlined">playlist_remove</span>
      <span class="label">Unwant All</span>
    </button>
  {/if}
</div>

<style>
  .season-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .season-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    height: 32px;
    padding: 0 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .season-action .material-icons-outlined {
    font-size: 16px;
  }

  .season-action.icon-only {
    width: 32px;
    padding: 0;
  }

  .season-action:hover {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: var(--accent);
  }

  .season-action.watched {
    color: var(--accent);
    border-color: var(--accent-dim);
  }

  @media (max-width: 768px) {
    .season-actions {
      gap: 4px;
    }

    .season-action {
      width: 32px;
      padding: 0;
    }

    .label {
      display: none;
    }
  }
</style>
