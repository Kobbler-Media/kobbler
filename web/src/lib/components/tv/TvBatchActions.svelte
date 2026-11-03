<script lang="ts">
  import { onMount } from 'svelte';
  import { getKobloadQualityProfiles, type KobloadQualityProfile } from '#lib/api/client.js';

  interface Props {
    selectedCount: number;
    loading: boolean;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onApply: (updates: { auto_want_mode?: string; quality_profile_id?: string | null }) => void;
  }

  let { selectedCount, loading, onSelectAll, onClearSelection, onApply }: Props = $props();

  let autoWantMode = $state('');
  let qualityProfileId = $state('');
  let qualityProfiles = $state<KobloadQualityProfile[]>([]);

  const hasChanges = $derived(autoWantMode !== '' || qualityProfileId !== '');

  onMount(async () => {
    try {
      qualityProfiles = await getKobloadQualityProfiles();
    } catch {
      // Kobload may not be configured
    }
  });

  function handleApply() {
    const updates: { auto_want_mode?: string; quality_profile_id?: string | null } = {};
    if (autoWantMode !== '') {
      updates.auto_want_mode = autoWantMode;
    }
    if (qualityProfileId === '__clear__') {
      updates.quality_profile_id = null;
    } else if (qualityProfileId !== '') {
      updates.quality_profile_id = qualityProfileId;
    }
    onApply(updates);
    autoWantMode = '';
    qualityProfileId = '';
  }
</script>

<div class="batch-actions">
  <div class="selection-info">
    <span>{selectedCount} selected</span>
    <button class="btn-link" type="button" onclick={onSelectAll}>Select all</button>
    <button class="btn-link" type="button" onclick={onClearSelection}>Clear</button>
  </div>
  <div class="batch-controls">
    <select class="batch-select" bind:value={autoWantMode}>
      <option value="">Auto-want...</option>
      <option value="none">None</option>
      <option value="new_only">New only</option>
      <option value="all">All episodes</option>
      <option value="fill_gaps">Fill gaps</option>
    </select>
    {#if qualityProfiles.length > 0}
      <select class="batch-select" bind:value={qualityProfileId}>
        <option value="">Quality profile...</option>
        <option value="__clear__">Clear profile</option>
        {#each qualityProfiles as profile (profile.id)}
          <option value={profile.id}>{profile.name}</option>
        {/each}
      </select>
    {/if}
    <button
      class="btn-apply"
      type="button"
      onclick={handleApply}
      disabled={loading || selectedCount === 0 || !hasChanges}
    >
      Apply
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
  }

  .selection-info {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
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
    gap: 8px;
  }

  .batch-select {
    appearance: none;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 6px;
    padding: 6px 28px 6px 10px;
    font-size: 12px;
    color: var(--ink);
    cursor: pointer;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23888'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 8px center;
  }

  .btn-apply {
    padding: 6px 16px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 12px;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-apply:hover:not(:disabled) {
    opacity: 0.9;
  }

  .btn-apply:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  @media (max-width: 768px) {
    .batch-actions {
      flex-direction: column;
      align-items: stretch;
      gap: 10px;
    }

    .batch-controls {
      flex-wrap: wrap;
    }

    .batch-select {
      flex: 1;
      min-width: 0;
    }
  }
</style>
