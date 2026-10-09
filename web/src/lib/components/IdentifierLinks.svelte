<script lang="ts">
  import { getProviderName } from '$lib/utils/format';

  interface Props {
    identifiers: Record<string, string>;
    entityType?: 'book' | 'author' | 'series' | 'person';
  }

  let { identifiers, entityType = 'book' }: Props = $props();

  // Get Hardcover URL using slug if available
  const getHardcoverUrl = (slug: string): string => {
    const pathSegment =
      entityType === 'author' ? 'authors' : entityType === 'series' ? 'series' : 'books';
    return `https://hardcover.app/${pathSegment}/${slug}`;
  };

  // URL mappings for known identifier types
  const getIdentifierUrl = (key: string, value: string): string | null => {
    const normalizedKey = key.toLowerCase();
    // Strip leading slashes from value to prevent double slashes in URLs
    const cleanValue = value.replace(/^\/+/, '');

    switch (normalizedKey) {
      case 'hardcover': {
        // Use slug if available, otherwise no link (numeric ID doesn't work in URLs)
        const slug = identifiers['hardcover_slug'];
        return slug ? getHardcoverUrl(slug) : null;
      }
      case 'isbn':
      case 'isbn13':
      case 'isbn10':
        return `https://openlibrary.org/isbn/${cleanValue}`;
      case 'goodreads':
        return `https://www.goodreads.com/book/show/${cleanValue}`;
      case 'asin':
        return `https://www.amazon.com/dp/${cleanValue}`;
      case 'google':
      case 'googlebooks':
        return `https://books.google.com/books?id=${cleanValue}`;
      case 'librarything':
        return `https://www.librarything.com/work/${cleanValue}`;
      case 'openlibrary':
        // OpenLibrary IDs look like OL1234W - link to the work page
        return `https://openlibrary.org/works/${cleanValue}`;
      case 'tmdb_id':
      case 'tmdb': {
        const tmdbType = entityType === 'person' ? 'person' : 'movie';
        return `https://www.themoviedb.org/${tmdbType}/${cleanValue}`;
      }
      case 'imdb_id':
      case 'imdb':
        return entityType === 'person'
          ? `https://www.imdb.com/name/${cleanValue}`
          : `https://www.imdb.com/title/${cleanValue}`;
      default:
        return null;
    }
  };

  // Internal keys that shouldn't be displayed as separate chips
  const hiddenKeys = ['hardcover_slug'];

  // Filter out empty identifiers and internal keys, then create entries
  const identifierEntries = $derived(
    Object.entries(identifiers)
      .filter(([key, value]) => value && String(value).trim() && !hiddenKeys.includes(key))
      .map(([key, value]) => ({
        key,
        value: String(value),
        displayName: getProviderName(key),
        url: getIdentifierUrl(key, value)
      }))
  );
</script>

{#if identifierEntries.length > 0}
  <div class="identifier-chips">
    {#each identifierEntries as entry (entry.key)}
      {#if entry.url}
        <a
          href={entry.url}
          target="_blank"
          rel="noopener noreferrer"
          class="identifier-chip clickable"
          title="Open in {entry.displayName}"
        >
          <span class="chip-label">{entry.displayName}</span>
          <span class="chip-value">{entry.value}</span>
          <svg viewBox="0 0 24 24" width="10" height="10" fill="currentColor" class="external-icon">
            <path
              d="M14 3v2h3.59l-9.83 9.83 1.41 1.41L19 6.41V10h2V3h-7zm-2 16H5V7h7V5H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-7h-2v7z"
            ></path>
          </svg>
        </a>
      {:else}
        <span class="identifier-chip">
          <span class="chip-label">{entry.displayName}</span>
          <span class="chip-value">{entry.value}</span>
        </span>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .identifier-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .identifier-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    text-decoration: none;
    color: inherit;
    transition: all 0.15s ease;
  }

  .identifier-chip.clickable {
    cursor: pointer;
  }

  .identifier-chip.clickable:hover {
    background: var(--accent);
    border-color: var(--accent);
    color: #1d130c;
  }

  .chip-label {
    color: var(--muted);
    font-weight: 500;
  }

  .identifier-chip.clickable:hover .chip-label {
    color: rgba(29, 19, 12, 0.7);
  }

  .chip-value {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .external-icon {
    opacity: 0.5;
    flex-shrink: 0;
  }

  .identifier-chip.clickable:hover .external-icon {
    opacity: 0.8;
  }
</style>
