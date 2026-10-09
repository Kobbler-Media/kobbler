<script lang="ts">
  import {
    createBookmark,
    listBookmarks,
    deleteBookmark,
    type AudiobookBookmark
  } from '$lib/api/client';
  import { getAudioPlayerState, seekToGlobal } from '$lib/stores/audioplayer.svelte';

  interface Props {
    bookId: string;
    currentFileId: string | null;
    currentTime: number;
  }

  let { bookId, currentFileId, currentTime }: Props = $props();

  const player = getAudioPlayerState();

  let bookmarks = $state<AudiobookBookmark[]>([]);
  let showNoteInput = $state(false);
  let noteText = $state('');
  let loading = $state(false);

  $effect(() => {
    if (bookId) {
      loadBookmarks();
    }
  });

  async function loadBookmarks() {
    try {
      bookmarks = await listBookmarks(bookId);
    } catch {
      bookmarks = [];
    }
  }

  async function handleAddBookmark() {
    if (!currentFileId || loading) return;
    loading = true;
    try {
      await createBookmark(bookId, {
        file_id: currentFileId,
        position_seconds: currentTime,
        note: noteText.trim() || undefined
      });
      noteText = '';
      showNoteInput = false;
      await loadBookmarks();
    } catch (e) {
      console.error('Failed to create bookmark:', e);
    } finally {
      loading = false;
    }
  }

  async function handleDelete(id: string) {
    try {
      await deleteBookmark(bookId, id);
      bookmarks = bookmarks.filter((b) => b.id !== id);
    } catch (e) {
      console.error('Failed to delete bookmark:', e);
    }
  }

  function formatTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) return '0:00';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const pad = (n: number) => n.toString().padStart(2, '0');
    return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  }

  function bookmarkGlobalTime(bm: AudiobookBookmark): number {
    if (bm.audiobook_file_id) {
      return player.globalTimeForFile(bm.audiobook_file_id, bm.position_seconds);
    }
    return bm.position_seconds;
  }
</script>

<div class="bookmark-section">
  <div class="bookmark-header">
    {#if showNoteInput}
      <div class="note-input-row">
        <input
          type="text"
          class="note-input"
          placeholder="Note (optional)"
          bind:value={noteText}
          onkeydown={(e) => {
            if (e.key === 'Enter') handleAddBookmark();
          }}
        />
        <button class="save-btn" type="button" onclick={handleAddBookmark} disabled={loading}>
          Save
        </button>
        <button
          class="cancel-btn"
          type="button"
          onclick={() => {
            showNoteInput = false;
            noteText = '';
          }}
        >
          Cancel
        </button>
      </div>
    {:else}
      <button class="add-btn" type="button" onclick={() => (showNoteInput = true)}>
        <span class="material-icons-outlined" style="font-size: 18px">bookmark_add</span>
        Add
      </button>
    {/if}
  </div>

  {#if bookmarks.length === 0}
    <p class="empty">No bookmarks yet</p>
  {:else}
    <ul class="bookmark-list">
      {#each bookmarks as bm (bm.id)}
        <li class="bookmark-item">
          <button
            class="bookmark-btn"
            type="button"
            onclick={() => seekToGlobal(bookmarkGlobalTime(bm))}
          >
            <span class="bm-time">{formatTime(bookmarkGlobalTime(bm))}</span>
            {#if bm.note}
              <span class="bm-note">{bm.note}</span>
            {/if}
          </button>
          <button
            class="delete-btn"
            type="button"
            onclick={() => handleDelete(bm.id)}
            aria-label="Delete bookmark"
          >
            <span class="material-icons-outlined" style="font-size: 16px">close</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .bookmark-section {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  .bookmark-header {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    margin-bottom: 8px;
    flex-shrink: 0;
  }

  .add-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary, #94a3b8);
    font-size: 13px;
    cursor: pointer;
  }

  .add-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-primary, #e2e8f0);
  }

  .note-input-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .note-input {
    flex: 1;
    padding: 4px 8px;
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.2);
    color: var(--text-primary, #e2e8f0);
    font-size: 13px;
  }

  .save-btn,
  .cancel-btn {
    padding: 4px 10px;
    border: none;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
  }

  .save-btn {
    background: var(--accent, #818cf8);
    color: white;
  }

  .cancel-btn {
    background: transparent;
    color: var(--text-secondary, #94a3b8);
  }

  .empty {
    color: var(--text-muted, #64748b);
    font-size: 13px;
    text-align: center;
    padding: 12px 0;
  }

  .bookmark-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .bookmark-item {
    display: flex;
    align-items: center;
    border-radius: 6px;
  }

  .bookmark-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .bookmark-btn {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: none;
    background: transparent;
    color: var(--text-primary, #e2e8f0);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .bm-time {
    font-variant-numeric: tabular-nums;
    color: var(--accent, #818cf8);
    font-weight: 500;
    min-width: 50px;
  }

  .bm-note {
    color: var(--text-secondary, #94a3b8);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .delete-btn {
    padding: 4px;
    border: none;
    background: transparent;
    color: var(--text-muted, #64748b);
    cursor: pointer;
    border-radius: 4px;
    opacity: 0;
  }

  .bookmark-item:hover .delete-btn {
    opacity: 1;
  }

  .delete-btn:hover {
    color: #ef4444;
    background: rgba(239, 68, 68, 0.1);
  }
</style>
