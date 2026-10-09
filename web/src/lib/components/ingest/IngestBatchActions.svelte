<script lang="ts">
  interface Props {
    selectedCount: number;
    loading: boolean;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onProcessSelected: () => void;
    onBatchReject: () => void;
    hasBatchReject: boolean;
  }

  let {
    selectedCount,
    loading,
    onSelectAll,
    onClearSelection,
    onProcessSelected,
    onBatchReject,
    hasBatchReject
  }: Props = $props();
</script>

<div class="batch-actions">
  <div class="selection-info">
    <span>{selectedCount} selected</span>
    <button class="btn-link" type="button" onclick={onSelectAll}>Select all</button>
    <button class="btn-link" type="button" onclick={onClearSelection}>Clear</button>
  </div>
  <div class="batch-buttons">
    <button
      class="btn"
      type="button"
      onclick={onProcessSelected}
      disabled={loading || selectedCount === 0}
    >
      Process Selected
    </button>
    <button
      class="btn btn-danger"
      type="button"
      onclick={onBatchReject}
      disabled={loading || selectedCount === 0 || !hasBatchReject}
    >
      Reject Selected
    </button>
  </div>
</div>

<style>
  .batch-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    margin-bottom: 16px;
  }

  .selection-info {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 13px;
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

  .batch-buttons {
    display: flex;
    gap: 8px;
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

  .btn-danger {
    background: var(--danger);
    color: white;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
