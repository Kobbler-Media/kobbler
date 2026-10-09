<script lang="ts">
  import type { Book, BookSelectHandler } from '$lib/types';
  import { getBookStatusBadge } from '$lib/utils/book-status';
  import { statusChip } from '$lib/utils/chip-mappings';
  import IconChip from '$lib/components/shared/IconChip.svelte';
  import MediaCard from '$lib/components/shared/MediaCard.svelte';

  interface Props {
    book: Book;
    selected: boolean;
    onSelect: BookSelectHandler;
  }

  let { book, selected, onSelect }: Props = $props();

  const isWanted = $derived(
    book.want_status === 'wanted' ||
      book.want_status === 'searching' ||
      book.want_status === 'downloading' ||
      book.want_status === 'ingesting' ||
      book.want_status === 'watching' ||
      book.want_status === 'search_failed'
  );

  const statusBadge = $derived(getBookStatusBadge(book));

  const showEbookIcon = $derived(
    (book.formats && book.formats.length > 0) ||
      (isWanted && (book.want_format === 'ebook' || book.want_format === 'both'))
  );
  const showAudiobookIcon = $derived(
    book.has_audiobook ||
      (isWanted && (book.want_format === 'audiobook' || book.want_format === 'both'))
  );

  const coverUrl = $derived(
    book.cover_url
      ? `${book.cover_url}?t=${encodeURIComponent(book.updated_at || '')}`
      : '/placeholder-cover.png'
  );
</script>

<MediaCard
  posterUrl={coverUrl}
  placeholderIcon="auto_stories"
  {selected}
  wanted={isWanted}
  itemId={book.id}
  onclick={(event) => onSelect(book.id, event)}
>
  {#snippet posterOverlay()}
    {#if statusBadge}
      {@const chip = statusChip(statusBadge.class)}
      <span class="chip-pos">
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
      </span>
    {/if}
    {#if showEbookIcon || showAudiobookIcon}
      <span class="format-icons">
        {#if showEbookIcon}
          <span class="material-icons-outlined" title="Ebook">auto_stories</span>
        {/if}
        {#if showAudiobookIcon}
          <span class="material-icons-outlined" title="Audiobook">headphones</span>
        {/if}
      </span>
    {/if}
  {/snippet}
  <strong class="card-title">{book.title}</strong>
  <div class="card-sub">{book.authors.map((a) => a.name).join(', ')}</div>
  {#if book.series_name}
    <div class="card-accent">
      {book.series_name}{book.series_index != null ? ` #${book.series_index}` : ''}
    </div>
  {/if}
</MediaCard>
