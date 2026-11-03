<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';

  interface Props {
    title?: string;
    /** Pre-loaded items (e.g., sibling episodes in same season) */
    items?: SearchSelectItem[];
    /** Async search function for finding target entities */
    searchFn?: (query: string) => Promise<SearchSelectItem[]>;
    /** Called with the selected target entity ID */
    onMove: (targetId: string) => Promise<void>;
    onClose: () => void;
  }

  let { title = 'Move File', items, searchFn, onMove, onClose }: Props = $props();

  let selectedId = $state('');
  let moving = $state(false);

  async function handleMove() {
    if (!selectedId) return;
    moving = true;
    try {
      await onMove(selectedId);
      showMessage('File moved');
      onClose();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to move file');
    } finally {
      moving = false;
    }
  }
</script>

<Modal {onClose} {title} overflowVisible>
  <div class="move-form">
    <label class="field-label" for="move-file-target">Move to</label>
    <SearchSelect
      bind:value={selectedId}
      placeholder="Search..."
      {items}
      {searchFn}
      id="move-file-target"
    />
  </div>

  {#snippet footer()}
    <button class="btn btn-cancel" type="button" onclick={onClose}>Cancel</button>
    <button
      class="btn btn-primary"
      type="button"
      onclick={handleMove}
      disabled={!selectedId || moving}
    >
      {#if moving}Moving...{:else}Move{/if}
    </button>
  {/snippet}
</Modal>

<style>
  .move-form {
    min-width: 350px;
  }

  .field-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: none;
  }

  .btn-cancel {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn-primary {
    background: var(--accent);
    color: var(--bg);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
