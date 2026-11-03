<script lang="ts">
  import { searchTranscript, type TranscriptSearchResult } from '#lib/api/client.js';
  import { getAudioPlayerState, seekToGlobal, formatTime } from '#lib/stores/audioplayer.svelte.js';

  let { bookId }: { bookId: string } = $props();

  const player = getAudioPlayerState();

  let query = $state('');
  let results = $state<TranscriptSearchResult[]>([]);
  let searching = $state(false);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  function handleInput() {
    if (debounceTimer) clearTimeout(debounceTimer);
    if (query.trim().length < 2) {
      results = [];
      return;
    }
    searching = true;
    debounceTimer = setTimeout(async () => {
      try {
        results = await searchTranscript(bookId, query.trim());
      } catch {
        results = [];
      }
      searching = false;
    }, 300);
  }

  function globalSeconds(result: TranscriptSearchResult): number | null {
    if (result.start_seconds == null) return null;
    return player.globalTimeForFile(result.file_id, result.start_seconds);
  }

  function seekToResult(result: TranscriptSearchResult) {
    const g = globalSeconds(result);
    if (g != null) seekToGlobal(g);
  }
</script>

<div class="transcript-search">
  <div class="search-input-wrap">
    <span class="material-icons-outlined search-icon">search</span>
    <input
      type="text"
      class="search-input"
      placeholder="Search transcript..."
      bind:value={query}
      oninput={handleInput}
    />
    {#if searching}
      <span class="spinner"></span>
    {/if}
  </div>

  {#if results.length > 0}
    <div class="search-results">
      {#each results as result (result.id)}
        <button type="button" class="result-item" onclick={() => seekToResult(result)}>
          <div class="result-meta">
            {#if result.start_seconds != null}
              <span class="result-time">{formatTime(globalSeconds(result) ?? 0)}</span>
            {/if}
            {#if result.chapter_title}
              <span class="result-chapter">{result.chapter_title}</span>
            {/if}
          </div>
          <div class="result-text">{result.text}</div>
        </button>
      {/each}
    </div>
  {:else if query.trim().length >= 2 && !searching}
    <div class="no-results">No matches found</div>
  {/if}
</div>

<style>
  .transcript-search {
    width: 100%;
  }

  .search-input-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 8px 12px;
  }

  .search-icon {
    font-size: 18px;
    opacity: 0.5;
    color: var(--ink);
  }

  .search-input {
    flex: 1;
    background: none;
    border: none;
    color: var(--ink);
    font-size: 0.9rem;
    outline: none;
  }

  .search-input::placeholder {
    color: var(--ink);
    opacity: 0.4;
  }

  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .search-results {
    margin-top: 8px;
    max-height: 300px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .result-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 12px;
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    color: var(--ink);
    transition: background 0.15s;
  }

  .result-item:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  .result-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.75rem;
    opacity: 0.6;
  }

  .result-time {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    color: var(--accent);
  }

  .result-chapter {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-text {
    font-size: 0.85rem;
    line-height: 1.4;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .no-results {
    padding: 12px;
    text-align: center;
    font-size: 0.85rem;
    opacity: 0.5;
    color: var(--ink);
  }
</style>
