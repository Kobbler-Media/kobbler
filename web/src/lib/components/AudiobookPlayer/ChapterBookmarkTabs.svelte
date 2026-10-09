<script lang="ts">
  import { untrack } from 'svelte';
  import ChapterList from './ChapterList.svelte';
  import BookmarkList from './BookmarkList.svelte';

  type Tab = 'chapters' | 'bookmarks';

  interface Props {
    bookId: string;
    currentFileId: string | null;
    currentTime: number;
    /** Whether the panel starts expanded. */
    defaultExpanded?: boolean;
  }

  let { bookId, currentFileId, currentTime, defaultExpanded = true }: Props = $props();

  let tab = $state<Tab>('chapters');
  let expanded = $state(untrack(() => defaultExpanded));
</script>

<div class="cb-tabs" class:expanded>
  <div class="cb-header">
    <div class="cb-tab-row">
      <button
        type="button"
        class="cb-tab"
        class:active={tab === 'chapters'}
        onclick={() => {
          if (tab === 'chapters' && expanded) {
            expanded = false;
          } else {
            tab = 'chapters';
            expanded = true;
          }
        }}
      >
        <span class="material-icons-outlined">queue_music</span>
        Chapters
      </button>
      <button
        type="button"
        class="cb-tab"
        class:active={tab === 'bookmarks'}
        onclick={() => {
          if (tab === 'bookmarks' && expanded) {
            expanded = false;
          } else {
            tab = 'bookmarks';
            expanded = true;
          }
        }}
      >
        <span class="material-icons-outlined">bookmark</span>
        Bookmarks
      </button>
    </div>
    <button
      type="button"
      class="cb-collapse"
      onclick={() => (expanded = !expanded)}
      title={expanded ? 'Collapse' : 'Expand'}
    >
      <span class="material-icons-outlined">
        {expanded ? 'expand_more' : 'expand_less'}
      </span>
    </button>
  </div>

  {#if expanded}
    <div class="cb-body">
      {#if tab === 'chapters'}
        <ChapterList />
      {:else}
        <BookmarkList {bookId} {currentFileId} {currentTime} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .cb-tabs {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-width: 0;
    min-height: 0;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    overflow: hidden;
  }

  .cb-tabs.expanded {
    flex: 1;
  }

  .cb-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 6px 6px 8px;
    border-bottom: 1px solid transparent;
    flex-shrink: 0;
  }

  .cb-tabs.expanded .cb-header {
    border-bottom-color: var(--ring);
  }

  .cb-tab-row {
    display: flex;
    gap: 2px;
  }

  .cb-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: none;
    background: transparent;
    color: var(--muted);
    font-size: 0.85rem;
    font-weight: 500;
    border-radius: 8px;
    cursor: pointer;
    transition:
      color 0.15s,
      background 0.15s;
  }

  .cb-tab .material-icons-outlined {
    font-size: 16px;
  }

  .cb-tab:hover {
    color: var(--ink);
    background: rgba(255, 255, 255, 0.05);
  }

  .cb-tab.active {
    color: var(--accent);
    background: rgba(255, 255, 255, 0.04);
  }

  .cb-collapse {
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .cb-collapse:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--ink);
  }

  .cb-body {
    flex: 1;
    min-height: 0;
    padding: 8px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .cb-body :global(> *) {
    flex: 1;
    min-height: 0;
  }
</style>
