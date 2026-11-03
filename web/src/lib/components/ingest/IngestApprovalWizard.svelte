<script lang="ts">
  import type { Book } from '#lib/types.js';
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import AuthorChips from '#lib/components/AuthorChips.svelte';
  import { formatSize } from './types';

  interface Props {
    items: Book[];
    currentIndex: number;
    loading: boolean;
    onApprove: (scope: string) => Promise<void>;
    onReject: () => Promise<void>;
    onSkip: () => void;
    onClose: () => void;
  }

  let { items, currentIndex, loading, onApprove, onReject, onSkip, onClose }: Props = $props();

  let wizardScope = $state<string>('all_users');

  const scopeItems: SearchSelectItem[] = [
    { value: 'all_users', label: 'All Devices' },
    { value: 'user', label: 'Per User' },
    { value: 'device', label: 'Specific Device' }
  ];

  const currentItem = $derived(currentIndex < items.length ? items[currentIndex] : null);

  const handleApprove = async () => {
    await onApprove(wizardScope);
  };
</script>

<button class="modal-backdrop" type="button" aria-label="Close wizard" onclick={onClose}></button>
<div class="wizard-panel">
  <div class="wizard-header">
    <h3>Process Items</h3>
    <div class="wizard-progress">
      {currentIndex + 1} of {items.length}
    </div>
    <button class="icon-btn" type="button" aria-label="Close wizard" onclick={onClose}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
        <path d="M6.4 5.6 5 7l5 5-5 5 1.4 1.4 5-5 5 5L17.8 17l-5-5 5-5-1.4-1.4-5 5-5-5z"></path>
      </svg>
    </button>
  </div>

  {#if currentItem}
    <div class="wizard-content">
      {#if currentItem.cover_url}
        <div class="wizard-cover">
          <img src={currentItem.cover_url} alt="Cover" loading="lazy" />
        </div>
      {/if}

      <div class="wizard-item-info">
        <h4>{currentItem.title || currentItem.ingest_info?.file_name || 'Untitled'}</h4>
        {#if currentItem.authors?.length}
          <div class="wizard-authors">
            <AuthorChips authors={currentItem.authors} maxDisplay={2} size="small" />
          </div>
        {/if}
        {#if currentItem.series_name}
          <p class="series-info">
            {currentItem.series_name}{#if currentItem.series_index}{` #${currentItem.series_index}`}{/if}
          </p>
        {/if}
        <div class="wizard-meta muted">
          <span
            >{currentItem.ingest_info?.source_format?.toUpperCase() ||
              currentItem.formats?.[0] ||
              ''}</span
          >
          <span>{formatSize(currentItem.ingest_info?.file_size ?? 0)}</span>
        </div>
      </div>

      {#if currentItem.description}
        <div class="wizard-description">
          <span class="section-label">Description</span>
          <p class="description-text">{currentItem.description}</p>
        </div>
      {/if}

      <div class="wizard-scope">
        <span class="section-label">Sync To</span>
        <div class="scope-select-wrapper">
          <SearchSelect items={scopeItems} bind:value={wizardScope} placeholder="All Devices" />
        </div>
      </div>
    </div>

    <div class="wizard-actions">
      <button class="btn" type="button" onclick={handleApprove} disabled={loading}>
        Approve & Next
      </button>
      <button class="btn alt" type="button" onclick={onSkip} disabled={loading}> Skip </button>
      <button class="btn btn-danger" type="button" onclick={onReject} disabled={loading}>
        Reject
      </button>
    </div>
  {/if}
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    border: none;
    z-index: 10;
  }

  .wizard-panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(90vw, 520px);
    max-height: 85vh;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 20px;
    padding: 20px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
    z-index: 11;
    display: grid;
    gap: 16px;
    overflow-y: auto;
  }

  .wizard-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .wizard-header h3 {
    margin: 0;
    flex: 1;
  }

  .wizard-progress {
    font-size: 12px;
    color: var(--muted);
    background: var(--ring);
    padding: 4px 10px;
    border-radius: 6px;
  }

  .icon-btn {
    background: transparent;
    border: none;
    padding: 6px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--muted);
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .wizard-content {
    display: grid;
    gap: 16px;
  }

  .wizard-cover {
    display: flex;
    justify-content: center;
    max-height: 200px;
  }

  .wizard-cover img {
    max-width: 100%;
    max-height: 200px;
    border-radius: 8px;
    object-fit: contain;
  }

  .wizard-item-info h4 {
    margin: 0 0 4px;
    font-size: 16px;
  }

  .wizard-item-info p {
    margin: 0;
    font-size: 13px;
  }

  .wizard-authors {
    margin-bottom: 4px;
  }

  .series-info {
    color: var(--accent);
    font-weight: 500;
  }

  .wizard-meta {
    display: flex;
    gap: 12px;
    font-size: 12px;
    margin-top: 8px;
  }

  .section-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .wizard-description {
    display: grid;
    gap: 6px;
  }

  .description-text {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    max-height: 100px;
    overflow-y: auto;
    color: var(--muted);
  }

  .wizard-scope {
    display: grid;
    gap: 6px;
  }

  .scope-select-wrapper {
    width: 160px;
  }

  .wizard-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.alt:hover {
    background: var(--ring);
  }

  .btn-danger {
    background: var(--danger);
    color: white;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .muted {
    color: var(--muted);
  }
</style>
