<script lang="ts">
  /**
   * FileActionsDropdown - A dropdown menu for file action buttons (replace/search).
   * Shows "Search releases" and "Reject & re-search" options.
   */

  interface Props {
    /** Called when user picks "Search releases" */
    onSearchReleases?: () => void;
    /** Called when user picks "Reject & re-search" */
    onRejectAndResearch?: () => void;
    /** Called when user picks "Replace" (delete file + re-want with exclusion) */
    onReplace?: () => void;
    /** Whether the reject/replace action is in progress */
    rejecting?: boolean;
    /** Icon to show on the trigger button */
    icon?: string;
    /** Size variant */
    size?: 'small' | 'normal';
  }

  let {
    onSearchReleases,
    onRejectAndResearch,
    onReplace,
    rejecting = false,
    icon = 'swap_horiz',
    size = 'small'
  }: Props = $props();

  let open = $state(false);

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest('.file-actions-dropdown')) {
      open = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      open = false;
    }
  }

  $effect(() => {
    if (open) {
      document.addEventListener('click', handleClickOutside);
      document.addEventListener('keydown', handleKeydown);
      return () => {
        document.removeEventListener('click', handleClickOutside);
        document.removeEventListener('keydown', handleKeydown);
      };
    }
  });
</script>

<div class="file-actions-dropdown" class:small={size === 'small'}>
  <button
    class="trigger-btn"
    type="button"
    title="Replace file"
    onclick={(e) => {
      e.stopPropagation();
      open = !open;
    }}
    disabled={rejecting}
  >
    {#if rejecting}
      <span class="material-icons-outlined">hourglass_empty</span>
    {:else}
      <span class="material-icons-outlined">{icon}</span>
    {/if}
  </button>

  {#if open}
    <div class="dropdown-menu">
      {#if onSearchReleases}
        <button
          class="dropdown-item"
          type="button"
          onclick={() => {
            open = false;
            onSearchReleases?.();
          }}
        >
          <span class="material-icons-outlined">search</span>
          Search releases
        </button>
      {/if}
      {#if onRejectAndResearch}
        <button
          class="dropdown-item danger"
          type="button"
          disabled={rejecting}
          onclick={() => {
            open = false;
            onRejectAndResearch?.();
          }}
        >
          <span class="material-icons-outlined">block</span>
          Reject &amp; re-search
        </button>
      {/if}
      {#if onReplace}
        <button
          class="dropdown-item danger"
          type="button"
          disabled={rejecting}
          onclick={() => {
            open = false;
            onReplace();
          }}
        >
          <span class="material-icons-outlined">find_replace</span>
          Replace file
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .file-actions-dropdown {
    position: relative;
    display: inline-flex;
  }

  .trigger-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .small .trigger-btn {
    width: 28px;
    height: 28px;
  }

  .small .trigger-btn .material-icons-outlined {
    font-size: 16px;
  }

  .trigger-btn:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .trigger-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dropdown-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    min-width: 180px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
    z-index: 50;
    overflow: hidden;
  }

  .dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 12px;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.1s;
    text-align: left;
  }

  .dropdown-item:hover:not(:disabled) {
    background: var(--bg-2);
  }

  .dropdown-item:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dropdown-item .material-icons-outlined {
    font-size: 16px;
    color: var(--muted);
  }

  .dropdown-item.danger:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.1);
    color: var(--danger);
  }

  .dropdown-item.danger:hover:not(:disabled) .material-icons-outlined {
    color: var(--danger);
  }
</style>
