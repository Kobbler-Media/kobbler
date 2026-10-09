<script lang="ts">
  import type { MetadataResult } from '$lib/api/client';
  import { getConfidenceColor } from './types';
  import { getProviderName, getProviderColor } from '$lib/utils/format';

  interface Props {
    result: MetadataResult;
    index: number;
    selected: boolean;
    multiSelectMode: boolean;
    mergePriority: number | null;
    onSelect: () => void;
  }

  let {
    result,
    index: _index,
    selected,
    multiSelectMode,
    mergePriority,
    onSelect
  }: Props = $props();
</script>

<button class="result-item" class:selected type="button" onclick={onSelect}>
  <div class="result-main">
    {#if multiSelectMode}
      <div class="merge-checkbox" class:checked={selected}>
        {#if mergePriority}
          <span class="priority-number">{mergePriority}</span>
        {:else}
          <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
            <path
              d="M19 5v14H5V5h14m0-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z"
            ></path>
          </svg>
        {/if}
      </div>
    {/if}
    {#if result.cover_url}
      <img src={result.cover_url} alt="" class="result-cover" />
    {:else}
      <div class="result-cover-placeholder"></div>
    {/if}
    <div class="result-info">
      <div class="result-title">{result.title}</div>
      {#if result.authors.length}
        <div class="result-authors muted">{result.authors.join(', ')}</div>
      {/if}
      <div class="result-meta muted">
        <span class="provider-badge" style="background: {getProviderColor(result.provider)}"
          >{getProviderName(result.provider)}</span
        >
        <span class="confidence-badge {getConfidenceColor(result.confidence)}">
          {Math.round(result.confidence * 100)}%
        </span>
        {#if result.rating}
          <span class="rating-badge">
            <svg viewBox="0 0 24 24" width="10" height="10" fill="currentColor">
              <path d="M12 17.3l6.2 3.7-1.6-7L22 9.2l-7.2-.6L12 2 9.2 8.6 2 9.2l5.4 4.8-1.6 7z"
              ></path>
            </svg>
            {result.rating.toFixed(1)}
          </span>
        {/if}
      </div>
    </div>
  </div>
  {#if !multiSelectMode && selected}
    <div class="result-details">
      {#if result.isbn}
        <div class="detail-row">
          <span class="detail-label">ISBN</span>
          <span>{result.isbn}</span>
        </div>
      {/if}
      {#if result.publisher}
        <div class="detail-row">
          <span class="detail-label">Publisher</span>
          <span>{result.publisher}</span>
        </div>
      {/if}
      {#if result.publish_date}
        <div class="detail-row">
          <span class="detail-label">Published</span>
          <span>{result.publish_date}</span>
        </div>
      {/if}
      {#if result.series}
        <div class="detail-row">
          <span class="detail-label">Series</span>
          <span>{result.series}{result.series_index ? ` #${result.series_index}` : ''}</span>
        </div>
      {/if}
      {#if result.page_count}
        <div class="detail-row">
          <span class="detail-label">Pages</span>
          <span>{result.page_count}</span>
        </div>
      {/if}
      {#if result.rating}
        <div class="detail-row">
          <span class="detail-label">Rating</span>
          <span>
            {result.rating.toFixed(1)}/5{result.rating_count
              ? ` (${result.rating_count.toLocaleString()} ratings)`
              : ''}
          </span>
        </div>
      {/if}
      {#if result.subjects && result.subjects.length > 0}
        <div class="detail-row">
          <span class="detail-label">Subjects</span>
          <div class="subjects-list">
            {#each result.subjects.slice(0, 5) as subject, i (`${i}-${subject}`)}
              <span class="subject-chip">{subject}</span>
            {/each}
            {#if result.subjects.length > 5}
              <span class="subject-more">+{result.subjects.length - 5} more</span>
            {/if}
          </div>
        </div>
      {/if}
      {#if result.description}
        <div class="detail-row">
          <span class="detail-label">Description</span>
          <p class="description">{result.description}</p>
        </div>
      {/if}
    </div>
  {/if}
</button>

<style>
  .result-item {
    display: grid;
    gap: 12px;
    padding: 12px;
    background: transparent;
    border: 1px solid var(--ring);
    border-radius: 10px;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background 0.15s;
    color: inherit;
    font: inherit;
  }

  .result-item:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.04);
  }

  .result-item.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  .result-main {
    display: flex;
    gap: 12px;
  }

  .result-cover {
    width: 50px;
    height: 70px;
    object-fit: cover;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .result-cover-placeholder {
    width: 50px;
    height: 70px;
    background: var(--ring);
    border-radius: 4px;
    flex-shrink: 0;
  }

  .result-info {
    flex: 1;
    min-width: 0;
  }

  .result-title {
    font-weight: 600;
    color: var(--ink);
  }

  .result-authors {
    font-size: 12px;
    margin-top: 2px;
  }

  .result-meta {
    display: flex;
    gap: 8px;
    margin-top: 6px;
    font-size: 11px;
  }

  .provider-badge {
    padding: 2px 6px;
    border-radius: 4px;
    color: #fff;
  }

  .confidence-badge {
    padding: 2px 6px;
    border-radius: 4px;
  }

  .confidence-badge.high {
    background: #22c55e;
    color: white;
  }

  .confidence-badge.medium {
    background: #f59e0b;
    color: white;
  }

  .confidence-badge.low {
    background: var(--ring);
  }

  .rating-badge {
    display: flex;
    align-items: center;
    gap: 3px;
    padding: 2px 6px;
    background: #fbbf24;
    color: #78350f;
    border-radius: 4px;
  }

  .result-details {
    display: grid;
    gap: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .detail-row {
    display: grid;
    gap: 2px;
  }

  .detail-label {
    font-size: 10px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .description {
    margin: 0;
    font-size: 12px;
    line-height: 1.4;
    max-height: 60px;
    overflow-y: auto;
  }

  .subjects-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 4px;
  }

  .subject-chip {
    padding: 2px 8px;
    background: var(--ring);
    border-radius: 12px;
    font-size: 11px;
    color: var(--ink);
  }

  .subject-more {
    padding: 2px 8px;
    font-size: 11px;
    color: var(--muted);
  }

  .merge-checkbox {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    border-radius: 6px;
    color: var(--muted);
    transition: background 0.15s;
  }

  .merge-checkbox.checked {
    background: var(--accent);
    color: white;
  }

  .priority-number {
    font-size: 13px;
    font-weight: 700;
  }

  .muted {
    color: var(--muted);
  }
</style>
