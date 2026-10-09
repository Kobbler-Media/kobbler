<script lang="ts">
  interface Props {
    label: string;
    owned: number;
    total: number;
    loading?: boolean;
    wantingAll?: boolean;
    onWantAllMissing?: () => void;
    onUnwantAll?: () => void;
    onSearch?: () => void;
  }

  let {
    label,
    owned,
    total,
    loading = false,
    wantingAll = false,
    onWantAllMissing,
    onUnwantAll,
    onSearch
  }: Props = $props();

  const progressPercent = $derived(total > 0 ? Math.round((owned / total) * 100) : 0);
</script>

<div class="completion-row">
  <div class="completion-bar-wrapper">
    <h4 class="section-title">{label}</h4>
    <div class="progress-track-slim">
      <div class="progress-fill" style="width: {progressPercent}%"></div>
    </div>
    <div class="completion-text">{owned} of {total} books owned</div>
  </div>
  <div class="section-actions">
    {#if onWantAllMissing}
      <button
        class="icon-btn small"
        type="button"
        title="Want all missing"
        disabled={loading || wantingAll}
        onclick={onWantAllMissing}
      >
        <span class="material-icons-outlined">playlist_add</span>
      </button>
    {/if}
    {#if onUnwantAll}
      <button
        class="icon-btn small"
        type="button"
        title="Unwant every book still being chased"
        disabled={loading || wantingAll}
        onclick={onUnwantAll}
      >
        <span class="material-icons-outlined">playlist_remove</span>
      </button>
    {/if}
    {#if onSearch}
      <button class="icon-btn small" type="button" title="Search releases" onclick={onSearch}>
        <span class="material-icons-outlined">search</span>
      </button>
    {/if}
  </div>
</div>

<style>
  .completion-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px 0;
  }

  .completion-bar-wrapper {
    flex: 1;
  }

  .section-title {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
  }

  .progress-track-slim {
    height: 10px;
    background: var(--ring);
    border-radius: 5px;
    overflow: hidden;
    margin-bottom: 4px;
  }

  .progress-track-slim .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 5px;
  }

  .completion-text {
    font-size: 12px;
    color: var(--muted);
  }

  .section-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }
</style>
