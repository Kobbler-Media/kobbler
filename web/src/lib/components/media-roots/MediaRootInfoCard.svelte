<script lang="ts">
  import type { MediaRoot } from '#lib/types.js';
  import { mediaTypeChip, mediaRootStatusChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import { formatDistanceToNow, formatDate } from '#lib/utils/format.js';
  import { setDefaultMediaRoot } from '#lib/api/client.js';

  interface Props {
    root: MediaRoot;
    loading?: boolean;
    onScan?: (id: string) => Promise<void>;
    onWriteSidecars?: (id: string) => Promise<void>;
    onWriteImages?: (id: string) => Promise<void>;
    onViewFiles?: (id: string) => void;
    onViewScanResults?: (id: string) => void;
    onReorganize?: (id: string) => void;
    onUpdate?: () => void;
  }

  let {
    root,
    loading = false,
    onScan,
    onWriteSidecars,
    onWriteImages,
    onViewFiles,
    onViewScanResults,
    onReorganize,
    onUpdate
  }: Props = $props();

  let settingDefault = $state(false);

  async function handleSetDefault() {
    settingDefault = true;
    try {
      await setDefaultMediaRoot(root.id);
      onUpdate?.();
    } catch (e) {
      console.error('Failed to set default:', e);
    } finally {
      settingDefault = false;
    }
  }

  const effectiveStatus = $derived(
    root.scan_status === 'error' && root.scan_error?.startsWith('Directory not found:')
      ? 'unavailable'
      : root.scan_status
  );
  const stats = $derived(root.stats);
  const totalFiles = $derived(stats?.total_files ?? 0);

  const statItems = $derived.by(() => {
    if (!stats) return [];
    return [
      { label: 'Added', count: stats.added, color: 'var(--accent)' },
      { label: 'Review', count: stats.review, color: 'var(--info, #3b82f6)' },
      { label: 'Identified', count: stats.identified, color: 'var(--accent)' },
      { label: 'Ambiguous', count: stats.ambiguous, color: 'var(--warning, #f59e0b)' },
      { label: 'Unmatched', count: stats.unmatched, color: 'var(--danger)' },
      { label: 'Pending', count: stats.pending, color: 'var(--muted)' },
      { label: 'Ignored', count: stats.ignored, color: 'var(--ring)' }
    ].filter((s) => s.count > 0);
  });

  const formatLastScan = (dateStr: string | null) => {
    if (!dateStr) return 'Never';
    return formatDistanceToNow(dateStr);
  };
</script>

<div class="info-card">
  <div class="detail-section">
    <span class="section-label">Type</span>
    <div class="type-row">
      <div class="type-badges">
        <IconChip chip={mediaTypeChip(root.media_type)} />
        {#if root.is_default}
          <span class="default-badge">Default</span>
        {/if}
      </div>
      {#if !root.is_default}
        <button
          class="icon-btn-small"
          type="button"
          title="Set as default"
          onclick={handleSetDefault}
          disabled={settingDefault}
        >
          <span class="material-icons-outlined">star_outline</span>
        </button>
      {/if}
    </div>
  </div>

  <div class="detail-section">
    <span class="section-label">Path</span>
    <code class="path-display">{root.path}</code>
  </div>

  <div class="detail-section">
    <span class="section-label">Settings</span>
    <div class="settings-grid">
      <span class="setting-label">Auto Add</span>
      <span class="setting-value">{root.auto_add ? 'Yes' : 'No'}</span>

      <span class="setting-label">Write Sidecars</span>
      <span class="setting-value">
        {root.write_sidecars ? 'Yes' : 'No'}
        {#if root.write_sidecars && onWriteSidecars}
          <button
            class="icon-btn-small"
            type="button"
            title="Write sidecars now"
            onclick={() => onWriteSidecars?.(root.id)}
            disabled={loading}
          >
            <span class="material-icons-outlined">description</span>
          </button>
        {/if}
      </span>

      <span class="setting-label">Write Images</span>
      <span class="setting-value">
        {root.write_images ? 'Yes' : 'No'}
        {#if root.write_images && onWriteImages}
          <button
            class="icon-btn-small"
            type="button"
            title="Write images now"
            onclick={() => onWriteImages?.(root.id)}
            disabled={loading}
          >
            <span class="material-icons-outlined">image</span>
          </button>
        {/if}
      </span>

      <span class="setting-label">Status</span>
      <span class="setting-value">
        <IconChip chip={mediaRootStatusChip(effectiveStatus)} />
        {#if !root.enabled}
          <IconChip chip={mediaRootStatusChip('disabled')} />
        {/if}
      </span>
    </div>
  </div>

  {#if root.scan_error?.startsWith('Directory not found:')}
    <div class="unavailable-alert">
      <span class="unavailable-icon material-icons-outlined">cloud_off</span>
      <div class="unavailable-body">
        <span class="unavailable-text">
          Path is not accessible. Processing is paused and will resume automatically when the path
          becomes available.
        </span>
        {#if root.deferred_ingest_count > 0}
          <span class="deferred-count">
            {root.deferred_ingest_count} download{root.deferred_ingest_count === 1 ? '' : 's'} waiting
            for this media root
          </span>
        {/if}
      </div>
    </div>
  {:else if root.scan_error}
    <div class="error-alert">
      <span class="error-icon">!</span>
      <span class="error-text">{root.scan_error}</span>
    </div>
  {/if}

  {#if stats && totalFiles > 0}
    <div class="detail-section">
      <div class="section-title-row">
        <span class="section-label">Files ({totalFiles})</span>
        <div class="section-actions">
          {#if onScan}
            <button
              class="icon-btn-small"
              type="button"
              title="Scan now"
              onclick={() => onScan?.(root.id)}
              disabled={loading || root.scan_status === 'scanning'}
            >
              <span class="material-icons-outlined">sync</span>
            </button>
          {/if}
          {#if onReorganize}
            <button
              class="icon-btn-small"
              type="button"
              title="Reorganize files"
              onclick={() => onReorganize?.(root.id)}
            >
              <span class="material-icons-outlined">drive_file_move</span>
            </button>
          {/if}
        </div>
      </div>
      {#if onViewFiles}
        <button
          class="stat-bar-btn"
          type="button"
          title="View files"
          onclick={() => onViewFiles?.(root.id)}
        >
          <div class="stat-bar">
            {#each statItems as item (item.label)}
              <div
                class="stat-segment"
                style="width: {(item.count / totalFiles) * 100}%; background: {item.color};"
                title="{item.label}: {item.count}"
              ></div>
            {/each}
          </div>
        </button>
      {:else}
        <div class="stat-bar">
          {#each statItems as item (item.label)}
            <div
              class="stat-segment"
              style="width: {(item.count / totalFiles) * 100}%; background: {item.color};"
              title="{item.label}: {item.count}"
            ></div>
          {/each}
        </div>
      {/if}
      <div class="stat-legend">
        {#each statItems as item (item.label)}
          <div class="legend-item">
            <span class="legend-dot" style="background: {item.color};"></span>
            <span class="legend-label">{item.label}</span>
            <span class="legend-count">{item.count}</span>
          </div>
        {/each}
      </div>
    </div>
  {:else if onScan || onReorganize}
    <div class="detail-section">
      <div class="section-title-row">
        <span class="section-label">Files</span>
        <div class="section-actions">
          {#if onScan}
            <button
              class="icon-btn-small"
              type="button"
              title="Scan now"
              onclick={() => onScan?.(root.id)}
              disabled={loading || root.scan_status === 'scanning'}
            >
              <span class="material-icons-outlined">sync</span>
            </button>
          {/if}
          {#if onReorganize}
            <button
              class="icon-btn-small"
              type="button"
              title="Reorganize files"
              onclick={() => onReorganize?.(root.id)}
            >
              <span class="material-icons-outlined">drive_file_move</span>
            </button>
          {/if}
        </div>
      </div>
      <span class="detail-value muted">No files found</span>
    </div>
  {/if}

  <div class="detail-section">
    <div class="section-title-row">
      <span class="section-label">Last Scan</span>
      {#if !root.auto_add && onViewScanResults}
        <div class="section-actions">
          <button
            class="icon-btn-small"
            type="button"
            title="View scan results"
            onclick={() => onViewScanResults?.(root.id)}
          >
            <span class="material-icons-outlined">checklist</span>
          </button>
        </div>
      {/if}
    </div>
    <span class="detail-value" title={root.last_scan_at ? formatDate(root.last_scan_at) : ''}>
      {formatLastScan(root.last_scan_at)}
    </span>
  </div>
</div>

<style>
  .info-card {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .detail-section {
    display: grid;
    gap: 6px;
  }

  .section-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .section-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-actions {
    display: flex;
    gap: 4px;
  }

  .detail-value {
    font-size: 13px;
  }

  .muted {
    color: var(--muted);
  }

  .type-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .type-badges {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .default-badge {
    display: inline-block;
    font-size: 10px;
    font-weight: 700;
    padding: 2px 8px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .path-display {
    font-size: 12px;
    padding: 8px 10px;
    background: var(--bg);
    border-radius: 8px;
    word-break: break-all;
    border: 1px solid var(--ring);
  }

  .settings-grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 16px;
    font-size: 13px;
  }

  .setting-label {
    color: var(--muted);
  }

  .setting-value {
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .unavailable-alert {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    background: color-mix(in srgb, var(--warning) 10%, transparent);
    border: 1px solid var(--warning);
    border-radius: 8px;
  }

  .unavailable-icon {
    flex-shrink: 0;
    font-size: 18px;
    color: var(--warning);
  }

  .unavailable-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .unavailable-text {
    font-size: 12px;
    color: var(--warning);
    line-height: 1.4;
  }

  .deferred-count {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
  }

  .error-alert {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    border: 1px solid var(--danger);
    border-radius: 8px;
  }

  .error-icon {
    flex-shrink: 0;
    width: 18px;
    height: 18px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: var(--danger);
    color: white;
    font-size: 11px;
    font-weight: 700;
  }

  .error-text {
    font-size: 12px;
    color: var(--danger);
    line-height: 1.4;
  }

  .stat-bar {
    display: flex;
    height: 8px;
    border-radius: 4px;
    overflow: hidden;
    background: var(--ring);
  }

  .stat-bar-btn {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
    border-radius: 4px;
    transition: opacity 0.15s;
  }

  .stat-bar-btn:hover {
    opacity: 0.8;
  }

  .stat-bar-btn .stat-bar {
    outline: 2px solid transparent;
    transition: outline-color 0.15s;
  }

  .stat-bar-btn:hover .stat-bar {
    outline-color: var(--muted);
  }

  .stat-segment {
    min-width: 3px;
    transition: width 0.2s;
  }

  .stat-legend {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 14px;
  }

  .legend-item {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
  }

  .legend-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .legend-label {
    color: var(--muted);
  }

  .legend-count {
    font-weight: 600;
    color: var(--ink);
  }

  .icon-btn-small {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    flex-shrink: 0;
  }

  .icon-btn-small:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn-small:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn-small :global(.material-icons-outlined) {
    font-size: 16px;
  }
</style>
