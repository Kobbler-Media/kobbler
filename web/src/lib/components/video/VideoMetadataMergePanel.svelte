<script lang="ts">
  import type { MovieSearchResult, TvSearchResult } from '$lib/types';
  import { getFieldOptions } from './video-merge-types';
  import { mergeVideoMetadata } from '$lib/api/client';

  type Result = MovieSearchResult | TvSearchResult;

  interface Props {
    mode: 'movie' | 'tv';
    results: Result[];
    selectedIndices: number[];
    onMergedResult: (result: Result) => void;
  }

  let { mode, results, selectedIndices, onMergedResult }: Props = $props();

  let fieldOverrides = $state<Partial<Record<string, number>>>({});
  let openDropdown = $state<string | null>(null);
  let merging = $state(false);
  let mergeError = $state<string | null>(null);

  const fields = $derived.by(() => {
    const base: { key: string; label: string }[] = [
      { key: 'title', label: 'Title' },
      { key: 'description', label: 'Description' },
      { key: 'poster_url', label: 'Poster' },
      { key: 'rating', label: 'Rating' },
      { key: 'year', label: 'Year' }
    ];
    if (mode === 'tv') {
      base.push({ key: 'network', label: 'Network' });
      base.push({ key: 'status', label: 'Status' });
    }
    return base;
  });

  // Build a preview poster from the first selected result that has one
  const previewPosterUrl = $derived.by(() => {
    if (selectedIndices.length < 2) return null;
    const posterOverrideIdx = fieldOverrides['poster_url'];
    if (posterOverrideIdx !== undefined) {
      const r = results[posterOverrideIdx] as Record<string, unknown>;
      return (r['poster_url'] as string | null) ?? null;
    }
    const opts = getFieldOptions('poster_url', selectedIndices, results);
    return opts.length > 0 ? (opts[0].value as string | null) : null;
  });

  function selectField(field: string, index: number) {
    fieldOverrides = { ...fieldOverrides, [field]: index };
    openDropdown = null;
  }

  function toggleDropdown(field: string) {
    openDropdown = openDropdown === field ? null : field;
  }

  function getDisplayValue(field: string, value: unknown): string {
    if (value === null || value === undefined) return '—';
    if (field === 'poster_url') return value ? 'Has poster' : 'No poster';
    if (field === 'rating' && typeof value === 'number') return value.toFixed(1);
    if (typeof value === 'string') {
      return value.length > 60 ? value.slice(0, 60) + '...' : value;
    }
    return String(value);
  }

  async function handleApply() {
    if (selectedIndices.length < 2) return;
    merging = true;
    mergeError = null;
    try {
      // Build ordered results based on selected indices (priority order)
      const orderedResults = selectedIndices.map((i) => results[i]);
      const merged = await mergeVideoMetadata(mode, orderedResults);
      onMergedResult(merged);
    } catch (e) {
      mergeError = e instanceof Error ? e.message : 'Merge failed';
    } finally {
      merging = false;
    }
  }
</script>

<div class="merge-panel">
  <h4 class="merge-title">Merge Preview</h4>

  {#if selectedIndices.length < 2}
    <p class="merge-hint">Select at least 2 results to merge.</p>
  {:else}
    <!-- Poster preview -->
    {#if previewPosterUrl}
      <div class="poster-preview">
        <img src={previewPosterUrl} alt="" class="poster-img" />
      </div>
    {/if}

    <!-- Field overrides -->
    <div class="fields">
      {#each fields as { key, label } (key)}
        {@const options = getFieldOptions(key, selectedIndices, results)}
        {#if options.length > 0}
          {@const activeIdx =
            fieldOverrides[key] !== undefined ? fieldOverrides[key]! : options[0].index}
          {@const activeResult = results[activeIdx]}
          <div class="field-row">
            <span class="field-label">{label}</span>
            <button
              class="field-value"
              type="button"
              onclick={() => toggleDropdown(key)}
              disabled={options.length <= 1}
            >
              <span class="field-provider">{activeResult.provider}</span>
              <span class="field-text">
                {getDisplayValue(key, (activeResult as Record<string, unknown>)[key])}
              </span>
              {#if options.length > 1}
                <span class="material-icons-outlined dropdown-icon">expand_more</span>
              {/if}
            </button>

            {#if openDropdown === key}
              <div class="dropdown">
                {#each options as opt (opt.index)}
                  <button
                    class="dropdown-item"
                    type="button"
                    onclick={() => selectField(key, opt.index)}
                  >
                    <span class="dropdown-provider">{opt.provider}</span>
                    <span class="dropdown-text">{getDisplayValue(key, opt.value)}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      {/each}
    </div>

    <!-- Genres (union from all selected, shown for preview) -->
    {@const allGenres = (() => {
      const seen = new Set<string>();
      const merged: string[] = [];
      for (const idx of selectedIndices) {
        for (const g of results[idx].genres || []) {
          const lower = g.toLowerCase();
          if (!seen.has(lower)) {
            seen.add(lower);
            merged.push(g);
          }
        }
      }
      return merged;
    })()}
    {#if allGenres.length > 0}
      <div class="genres-section">
        <span class="field-label">Genres</span>
        <div class="genre-tags">
          {#each allGenres as genre, i (`${i}-${genre}`)}
            <span class="genre-tag">{genre}</span>
          {/each}
        </div>
      </div>
    {/if}

    {#if mergeError}
      <p class="merge-error">{mergeError}</p>
    {/if}

    <!-- Apply button -->
    <button class="btn-apply" type="button" onclick={handleApply} disabled={merging}>
      {merging ? 'Merging...' : 'Use Merged Result'}
    </button>
  {/if}
</div>

<style>
  .merge-panel {
    padding: 16px;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
  }

  .merge-title {
    margin: 0 0 12px;
    font-size: 13px;
    font-weight: 700;
    color: var(--ink);
  }

  .merge-hint {
    font-size: 12px;
    color: var(--muted);
    margin: 0;
  }

  .poster-preview {
    display: flex;
    justify-content: center;
    margin-bottom: 12px;
  }

  .poster-img {
    width: 80px;
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-row {
    position: relative;
  }

  .field-label {
    display: block;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
    margin-bottom: 3px;
  }

  .field-value {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 12px;
    cursor: pointer;
    text-align: left;
    font: inherit;
    transition: border-color 0.15s;
  }

  .field-value:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .field-value:disabled {
    cursor: default;
    opacity: 0.8;
  }

  .field-provider {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    color: var(--accent);
    flex-shrink: 0;
  }

  .field-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dropdown-icon {
    font-size: 16px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 10;
    margin-top: 2px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
    overflow: hidden;
  }

  .dropdown-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    cursor: pointer;
    text-align: left;
    font: inherit;
    transition: background 0.15s;
  }

  .dropdown-item:hover {
    background: var(--bg-2);
  }

  .dropdown-provider {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    color: var(--accent);
    flex-shrink: 0;
  }

  .dropdown-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .genres-section {
    margin-top: 12px;
  }

  .genre-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .genre-tag {
    font-size: 10px;
    padding: 2px 8px;
    border-radius: 4px;
    background: var(--bg-2);
    color: var(--muted);
    font-weight: 600;
  }

  .merge-error {
    margin-top: 8px;
    font-size: 12px;
    color: var(--danger, #ef4444);
  }

  .btn-apply {
    margin-top: 16px;
    width: 100%;
    padding: 10px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-apply:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-apply:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
