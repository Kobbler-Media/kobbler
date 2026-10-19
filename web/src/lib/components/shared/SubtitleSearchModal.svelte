<script lang="ts">
  import {
    searchMovieSubtitles,
    searchEpisodeSubtitles,
    downloadSubtitle,
    downloadSubtitleToSession
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type { SubtitleSearchResult, PlaybackSubtitleTrack } from '$lib/types';

  interface Props {
    mediaType: 'movie' | 'episode';
    entityId: string;
    fileId: string;
    /** When set, downloads go to this playback session instead of the entity file. */
    sessionId?: string;
    onClose: () => void;
    onDownloaded?: (track?: PlaybackSubtitleTrack) => void;
  }

  let { mediaType, entityId, fileId, sessionId, onClose, onDownloaded }: Props = $props();

  let languages = $state('eng');
  let forcedOnly = $state(false);
  let searching = $state(false);
  let results = $state<SubtitleSearchResult[]>([]);
  let searched = $state(false);
  let downloadingId = $state<string | null>(null);

  const sortedResults = $derived(
    [...results].sort((a, b) => (b.download_count ?? 0) - (a.download_count ?? 0))
  );

  async function handleSearch() {
    const langs = languages
      .split(',')
      .map((l) => l.trim())
      .filter(Boolean);
    if (langs.length === 0) {
      showMessage('Enter at least one language code');
      return;
    }
    searching = true;
    searched = false;
    results = [];
    try {
      const searchFn = mediaType === 'movie' ? searchMovieSubtitles : searchEpisodeSubtitles;
      results = await searchFn(entityId, { languages: langs, forced_only: forcedOnly });
      searched = true;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Subtitle search failed');
    } finally {
      searching = false;
    }
  }

  async function handleDownload(result: SubtitleSearchResult) {
    downloadingId = result.id;
    try {
      if (sessionId) {
        // Download directly to the active playback session
        const track = await downloadSubtitleToSession(sessionId, {
          provider: result.provider,
          subtitle_id: result.id,
          language: result.language,
          is_forced: result.is_forced,
          is_hearing_impaired: result.is_hearing_impaired
        });
        showMessage('Subtitle downloaded');
        onDownloaded?.(track);
      } else {
        // Download and save to the media entity file
        const fileKey = mediaType === 'movie' ? 'movie_file_id' : 'episode_file_id';
        await downloadSubtitle({
          provider: result.provider,
          subtitle_id: result.id,
          [fileKey]: fileId,
          language: result.language,
          is_forced: result.is_forced,
          is_hearing_impaired: result.is_hearing_impaired
        });
        showMessage('Subtitle downloaded');
        onDownloaded?.();
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Download failed');
    } finally {
      downloadingId = null;
    }
  }

  function formatLang(code: string): string {
    try {
      return new Intl.DisplayNames(['en'], { type: 'language' }).of(code) ?? code;
    } catch {
      return code;
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={onClose}>
  <div class="modal" onclick={(e) => e.stopPropagation()}>
    <div class="modal-header">
      <h3>Search Subtitles</h3>
      <button class="close-btn" type="button" onclick={onClose}>
        <span class="material-icons-outlined">close</span>
      </button>
    </div>

    <div class="search-form">
      <label>
        Languages (comma-separated ISO codes)
        <input
          type="text"
          bind:value={languages}
          placeholder="eng, spa, fre"
          onkeydown={(e) => e.key === 'Enter' && handleSearch()}
        />
      </label>
      <label class="checkbox-row">
        <input type="checkbox" bind:checked={forcedOnly} />
        Forced subtitles only
      </label>
      <button class="btn" type="button" onclick={handleSearch} disabled={searching}>
        {searching ? 'Searching...' : 'Search'}
      </button>
    </div>

    <div class="results">
      {#if searching}
        <div class="results-status">Searching providers...</div>
      {:else if searched && results.length === 0}
        <div class="results-status muted">No subtitles found</div>
      {:else if results.length > 0}
        <div class="results-count muted">
          {results.length} result{results.length !== 1 ? 's' : ''}
        </div>
        <div class="results-list">
          {#each sortedResults as result (result.provider + ':' + result.id)}
            <div class="result-row">
              <div class="result-info">
                <span class="result-lang">{formatLang(result.language)}</span>
                <span class="result-format">{result.format.toUpperCase()}</span>
                {#if result.is_forced}
                  <span class="badge forced">Forced</span>
                {/if}
                {#if result.is_hearing_impaired}
                  <span class="badge hi">HI</span>
                {/if}
                <span class="result-meta muted">
                  <MetaLine
                    items={[
                      result.provider,
                      result.download_count
                        ? `${result.download_count.toLocaleString()} downloads`
                        : null,
                      result.rating ? result.rating.toFixed(1) : null
                    ]}
                  />
                </span>
              </div>
              <button
                class="btn small"
                type="button"
                onclick={() => handleDownload(result)}
                disabled={downloadingId !== null}
              >
                {downloadingId === result.id ? 'Downloading...' : 'Download'}
              </button>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 16px;
  }

  .modal {
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 16px;
    width: 100%;
    max-width: 560px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--ring);
  }

  .modal-header h3 {
    margin: 0;
    font-size: 16px;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 8px;
  }

  .close-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .search-form {
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    border-bottom: 1px solid var(--ring);
  }

  .search-form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .search-form input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .checkbox-row {
    flex-direction: row !important;
    align-items: center;
    gap: 8px !important;
  }

  .results {
    flex: 1;
    overflow-y: auto;
    padding: 12px 20px 16px;
  }

  .results-status {
    text-align: center;
    padding: 24px 0;
    font-size: 13px;
    color: var(--muted);
  }

  .results-count {
    font-size: 11px;
    margin-bottom: 8px;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .result-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
  }

  .result-info {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }

  .result-lang {
    font-weight: 600;
    font-size: 13px;
  }

  .result-format {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--ring);
    font-weight: 600;
  }

  .badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    font-weight: 600;
  }

  .badge.forced {
    background: var(--accent-dim, rgba(232, 168, 124, 0.15));
    color: var(--accent);
  }

  .badge.hi {
    background: rgba(100, 149, 237, 0.15);
    color: cornflowerblue;
  }

  .result-meta {
    font-size: 11px;
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
    flex-shrink: 0;
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn.small {
    padding: 5px 10px;
    font-size: 12px;
    border-radius: 8px;
  }

  @media (max-width: 768px) {
    .modal {
      max-height: 90vh;
    }

    .result-row {
      flex-direction: column;
      align-items: stretch;
    }

    .result-info {
      margin-bottom: 4px;
    }
  }
</style>
