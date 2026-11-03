<script lang="ts">
  import { onMount } from 'svelte';
  import type { MediaRoot } from '#lib/types.js';
  import { listMediaRoots, moveBookToMediaRoot } from '#lib/api/client.js';
  import { Modal, SearchSelect } from '@koblibs/ui';

  interface Props {
    bookId: string;
    bookTitle: string;
    onClose: () => void;
    onMoved: () => void;
  }

  let { bookId, bookTitle, onClose, onMoved }: Props = $props();

  let mediaRoots = $state<MediaRoot[]>([]);
  let selectedRootId = $state<string | null>(null);
  let loading = $state(false);
  let loadingRoots = $state(true);
  let error = $state('');

  onMount(async () => {
    try {
      const roots = await listMediaRoots();
      mediaRoots = roots.filter(
        (r) => (r.media_type === 'book' || r.media_type === 'audiobook') && r.enabled
      );
      if (mediaRoots.length === 1) {
        selectedRootId = mediaRoots[0].id;
      }
    } catch {
      error = 'Failed to load media roots';
    } finally {
      loadingRoots = false;
    }
  });

  const handleMove = async () => {
    if (!selectedRootId) return;
    loading = true;
    error = '';
    try {
      const result = await moveBookToMediaRoot(bookId, selectedRootId);
      if (result.errors.length > 0) {
        error = result.errors.join('; ');
        if (result.moved_files === 0) return;
      }
      onMoved();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Move failed';
    } finally {
      loading = false;
    }
  };

  const selectedRoot = $derived(mediaRoots.find((r) => r.id === selectedRootId));
</script>

<Modal title="Move to Media Root" {onClose}>
  <p class="modal-description">
    Move files for <strong>{bookTitle}</strong> into a media root directory.
  </p>

  {#if loadingRoots}
    <p class="loading-text">Loading media roots...</p>
  {:else if mediaRoots.length === 0}
    <p class="error-text">No enabled book media roots found.</p>
  {:else}
    <label class="field-label" for="media-root-select">Target media root</label>
    <SearchSelect
      id="media-root-select"
      items={mediaRoots.map((r) => ({
        value: r.id,
        label: `${r.name}${r.is_default ? ' (default)' : ''} — ${r.path}`
      }))}
      value={selectedRootId ?? ''}
      onchange={(v) => {
        selectedRootId = v || null;
      }}
      placeholder="Select a media root..."
    />

    {#if selectedRoot}
      <div class="template-info">
        <span class="label">Naming template:</span>
        <code>{selectedRoot.naming_template || '{author}/{title}/{title}.{ext}'}</code>
      </div>
    {/if}
  {/if}

  {#if error}
    <p class="error-text">{error}</p>
  {/if}

  {#snippet footer()}
    <button class="btn alt" type="button" onclick={onClose} disabled={loading}>Cancel</button>
    <button
      class="btn"
      type="button"
      onclick={handleMove}
      disabled={loading || !selectedRootId || loadingRoots}
    >
      {loading ? 'Moving...' : 'Move'}
    </button>
  {/snippet}
</Modal>

<style>
  .modal-description {
    margin: 0 0 16px;
    font-size: 13px;
    color: var(--muted);
  }

  .modal-description strong {
    color: var(--fg);
  }

  .field-label {
    display: block;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
    margin-bottom: 6px;
  }

  .template-info {
    margin-top: 12px;
    padding: 8px 10px;
    background: var(--bg-secondary);
    border-radius: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .template-info .label {
    margin-right: 4px;
  }

  .template-info code {
    font-family: var(--font-mono, monospace);
    color: var(--fg);
  }

  .loading-text {
    font-size: 13px;
    color: var(--muted);
  }

  .error-text {
    font-size: 13px;
    color: var(--error, #e53935);
    margin-top: 8px;
  }
</style>
