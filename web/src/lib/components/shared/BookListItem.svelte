<script lang="ts">
  import MetaLine from './MetaLine.svelte';
  import type { Book } from '$lib/types';
  import {
    getBookListStatusBadge,
    shouldShowWantButton,
    shouldShowSkipButton
  } from '$lib/utils/book-status';

  interface Props {
    book: Book;
    onBookClick?: (book: Book) => void;
    onWant?: (book: Book) => void;
    onSkip?: (book: Book) => void;
    wantingBookId?: string | null;
    showSeriesIndex?: boolean;
    showPubdate?: boolean;
  }

  let {
    book,
    onBookClick,
    onWant,
    onSkip,
    wantingBookId = null,
    showSeriesIndex = false,
    showPubdate = false
  }: Props = $props();

  const isOwned = $derived(book.is_in_library !== false);
  const badge = $derived(getBookListStatusBadge(book));
  const statusClass = $derived(badge.class);
  const statusText = $derived(badge.text);
  const showPulse = $derived(
    statusClass === 'searching' || statusClass === 'downloading' || statusClass === 'watching'
  );
  const showWant = $derived(!isOwned && shouldShowWantButton(statusClass));
  const showSkip = $derived(!isOwned && shouldShowSkipButton(statusClass));
</script>

<div
  class="book-list-item"
  class:not-owned={!isOwned}
  class:clickable={!!onBookClick}
  role="button"
  tabindex="0"
  onclick={() => onBookClick?.(book)}
  onkeydown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onBookClick?.(book);
    }
  }}
>
  <div
    class="book-list-thumb"
    style={book.cover_url ? `background-image: url(${book.cover_url});` : ''}
  ></div>
  <div class="book-list-info">
    <div class="book-list-title">{book.title}</div>
    <div class="book-list-meta">
      <MetaLine
        items={[
          showSeriesIndex && book.series_index && `#${book.series_index}`,
          showPubdate && book.pubdate && new Date(book.pubdate).getFullYear()
        ]}
      />
    </div>
  </div>
  <span class="status-pill {statusClass}" class:pulse={showPulse}>{statusText}</span>
  {#if showWant || showSkip}
    <div class="action-btns">
      {#if showWant}
        <button
          class="want-btn"
          type="button"
          disabled={wantingBookId === book.id}
          onclick={(e) => {
            e.stopPropagation();
            onWant?.(book);
          }}>Want</button
        >
      {/if}
      {#if showSkip}
        <button
          class="skip-btn"
          type="button"
          disabled={wantingBookId === book.id}
          onclick={(e) => {
            e.stopPropagation();
            onSkip?.(book);
          }}>Skip</button
        >
      {/if}
    </div>
  {/if}
</div>

<style>
  .book-list-item {
    display: grid;
    grid-template-columns: 32px 1fr auto auto;
    gap: 12px;
    padding: 10px 12px;
    align-items: center;
    border-top: 1px solid var(--ring);
    background: none;
    border-left: none;
    border-right: none;
    border-bottom: none;
    width: 100%;
    text-align: left;
    font: inherit;
    color: inherit;
  }

  .book-list-item.clickable {
    cursor: pointer;
  }

  .book-list-item.clickable:hover {
    background: var(--card);
  }

  .book-list-item.not-owned {
    opacity: 0.7;
  }

  .book-list-thumb {
    width: 32px;
    height: 48px;
    border-radius: 4px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg);
  }

  .book-list-info {
    min-width: 0;
  }

  .book-list-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .book-list-meta {
    font-size: 11px;
    color: var(--muted);
  }

  .action-btns {
    display: flex;
    gap: 4px;
  }

  .want-btn {
    flex-shrink: 0;
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid var(--accent);
    background: transparent;
    color: var(--accent);
    cursor: pointer;
  }

  .want-btn:hover {
    background: var(--accent);
    color: #1d130c;
  }

  .want-btn:disabled,
  .skip-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .skip-btn {
    flex-shrink: 0;
    font-size: 11px;
    padding: 4px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .skip-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
  }
</style>
