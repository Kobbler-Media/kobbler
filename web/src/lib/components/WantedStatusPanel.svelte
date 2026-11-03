<script lang="ts">
  import {
    getWantedStatus,
    searchWantedNow,
    grabHeldMatch,
    rejectWantedDownload
  } from '#lib/api/client.js';
  import type { WantedStatusResponse, WantedMatch } from '#lib/types.js';
  import { wantedPanelChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';

  interface Props {
    wantedId: string;
    onStatusChange?: () => void;
    onSearchReleases?: () => void;
  }

  let { wantedId, onStatusChange, onSearchReleases }: Props = $props();

  let status = $state<WantedStatusResponse | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let actionInProgress = $state<string | null>(null);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function fetchStatus() {
    try {
      status = await getWantedStatus(wantedId);
      error = null;
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : 'Failed to load status';
    } finally {
      loading = false;
    }
  }

  function startPolling() {
    stopPolling();
    pollTimer = setInterval(async () => {
      await fetchStatus();
      // Stop polling once action completes
      if (actionInProgress && status?.pattern) {
        const s = status.pattern.status;
        if (
          (actionInProgress === 'searching' && s !== 'searching') ||
          (actionInProgress === 'grabbing' && s !== 'held')
        ) {
          actionInProgress = null;
          stopPolling();
          onStatusChange?.();
        }
      }
    }, 3000);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function handleSearchNow() {
    actionInProgress = 'searching';
    try {
      await searchWantedNow(wantedId);
      startPolling();
    } catch (e: unknown) {
      actionInProgress = null;
      showMessage(e instanceof Error ? e.message : 'Search failed');
    }
  }

  async function handleGrabNow() {
    actionInProgress = 'grabbing';
    try {
      await grabHeldMatch(wantedId);
      startPolling();
    } catch (e: unknown) {
      actionInProgress = null;
      showMessage(e instanceof Error ? e.message : 'Grab failed');
    }
  }

  async function handleReject() {
    actionInProgress = 'rejecting';
    try {
      await rejectWantedDownload(wantedId);
      await fetchStatus();
      actionInProgress = null;
      onStatusChange?.();
    } catch (e: unknown) {
      actionInProgress = null;
      showMessage(e instanceof Error ? e.message : 'Reject failed');
    }
  }

  function formatTimeRemaining(holdUntil: string): string {
    const diff = new Date(holdUntil).getTime() - Date.now();
    if (diff <= 0) return 'expiring soon';
    const hours = Math.floor(diff / 3600000);
    const minutes = Math.floor((diff % 3600000) / 60000);
    if (hours > 0) return `${hours}h ${minutes}m remaining`;
    return `${minutes}m remaining`;
  }

  // Fetch on mount
  $effect(() => {
    fetchStatus();
    return () => stopPolling();
  });
</script>

{#if loading}
  <div class="wanted-status-panel loading">
    <div class="skeleton-line"></div>
    <div class="skeleton-line short"></div>
  </div>
{:else if error}
  <div class="wanted-status-panel error">
    <span class="status-text">{error}</span>
    <button class="btn btn-sm" onclick={fetchStatus}>Retry</button>
  </div>
{:else if status?.not_tracked}
  <div class="wanted-status-panel not-tracked">
    <span class="status-text">Not currently tracked by download system</span>
    {#if onSearchReleases}
      <div class="status-actions">
        <button class="btn btn-sm" onclick={onSearchReleases}>Search Releases</button>
      </div>
    {/if}
  </div>
{:else if status?.pattern}
  {@const pat = status.pattern}
  {@const heldMatch = status.matches.find((m: WantedMatch) => m.held)}

  <div class="wanted-status-panel" data-status={pat.status}>
    <div class="status-header">
      {#if pat.status === 'held' && heldMatch}
        {@const chip = wantedPanelChip('held')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">
          Torrent found — waiting for usenet ({formatTimeRemaining(heldMatch.hold_until ?? '')})
        </span>
      {:else if pat.status === 'searching'}
        {@const chip = wantedPanelChip('searching')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">Monitoring for new releases</span>
      {:else if pat.status === 'downloading'}
        {@const chip = wantedPanelChip('downloading')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">Download in progress</span>
      {:else if pat.status === 'upgrading'}
        {@const chip = wantedPanelChip('upgrading')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">Downloaded — looking for better quality</span>
      {:else if pat.status === 'satisfied'}
        {@const chip = wantedPanelChip('satisfied')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">Download complete</span>
      {:else if pat.status === 'paused'}
        {@const chip = wantedPanelChip('paused')}
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
        <span class="status-text">Monitoring paused</span>
      {/if}
    </div>

    {#if heldMatch && pat.status === 'held'}
      <div class="held-match-info">
        <span class="match-title">{heldMatch.release_title}</span>
        <span class="match-score">Score: {(heldMatch.match_score * 100).toFixed(0)}%</span>
      </div>
    {/if}

    <div class="status-actions">
      {#if actionInProgress === 'searching'}
        <button class="btn btn-sm" disabled>Searching...</button>
      {:else if actionInProgress === 'grabbing'}
        <button class="btn btn-sm" disabled>Grabbing...</button>
      {:else if actionInProgress === 'rejecting'}
        <button class="btn btn-sm" disabled>Rejecting...</button>
      {:else}
        {#if pat.status === 'held'}
          <button class="btn btn-sm btn-primary" onclick={handleGrabNow}>Grab Now</button>
          <button class="btn btn-sm" onclick={handleSearchNow}>Search Again</button>
        {/if}
        {#if pat.status === 'searching'}
          <button class="btn btn-sm btn-primary" onclick={handleSearchNow}>Search Now</button>
        {/if}
        {#if pat.status === 'downloading'}
          <button class="btn btn-sm btn-danger" onclick={handleReject}
            >Reject & Find Alternative</button
          >
        {/if}
        {#if onSearchReleases}
          <button class="btn btn-sm" onclick={onSearchReleases}>Search Releases</button>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .wanted-status-panel {
    padding: 0.75rem 1rem;
    border-radius: 0.5rem;
    background: var(--surface-1, #f8f9fa);
    border: 1px solid var(--border-color, #e0e0e0);
    margin-top: 0.75rem;
  }

  .wanted-status-panel.loading {
    opacity: 0.6;
  }

  .skeleton-line {
    height: 0.875rem;
    background: var(--skeleton-bg, #dee2e6);
    border-radius: 0.25rem;
    margin-bottom: 0.5rem;
    animation: pulse 1.5s ease-in-out infinite;
  }

  .skeleton-line.short {
    width: 60%;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 0.4;
    }
    50% {
      opacity: 0.8;
    }
  }

  .status-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .status-text {
    font-size: 0.875rem;
    color: var(--text-secondary, #6c757d);
  }

  .held-match-info {
    margin-top: 0.5rem;
    padding: 0.5rem;
    background: var(--surface-2, #fff);
    border-radius: 0.25rem;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.8125rem;
  }

  .match-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .match-score {
    color: var(--text-secondary, #6c757d);
    white-space: nowrap;
  }

  .status-actions {
    margin-top: 0.5rem;
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .btn {
    display: inline-flex;
    align-items: center;
    padding: 0.25rem 0.75rem;
    border: 1px solid var(--border-color, #ced4da);
    border-radius: 0.25rem;
    background: var(--surface-2, #fff);
    color: var(--text-primary, #212529);
    font-size: 0.8125rem;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .btn:hover:not(:disabled) {
    background: var(--surface-hover, #e9ecef);
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--primary, #0d6efd);
    color: #fff;
    border-color: var(--primary, #0d6efd);
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--primary-hover, #0b5ed7);
  }

  .btn-danger {
    background: var(--danger, #dc3545);
    color: #fff;
    border-color: var(--danger, #dc3545);
  }

  .btn-danger:hover:not(:disabled) {
    background: var(--danger-hover, #bb2d3b);
  }

  .btn-sm {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
  }

  .wanted-status-panel.error {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
</style>
