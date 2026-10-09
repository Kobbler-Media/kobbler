<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import ShelvesPage from '$lib/components/pages/ShelvesPage.svelte';
  import { getLibraryState } from '$lib/stores/library.svelte';
  import {
    loadShelves,
    loadMoreShelves,
    loadShelfItems,
    showMessage
  } from '$lib/context/app.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import * as api from '$lib/api/client';
  import type { Book } from '$lib/types';

  // Store state
  const library = getLibraryState();

  // Derived state
  const shelves = $derived(library.shelves);
  const shelvesNextCursor = $derived(library.shelvesNextCursor);
  const shelvesTotal = $derived(library.shelvesTotal);
  const shelfItems = $derived(library.shelfItems);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);

  // URL state
  const urlSelected = $derived($page.url.searchParams.get('selected'));

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadShelves();
  };

  onMount(async () => {
    window.addEventListener('scrolltotop', handleScrollToTop);
    await loadShelves();

    // Restore selection from URL
    if (urlSelected) {
      await loadShelfItems(urlSelected);
    }
  });

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
  });

  // Handlers
  const handleSelectShelf = async (shelfId: string | null) => {
    updateUrlParams({ selected: shelfId });
    if (shelfId) {
      await loadShelfItems(shelfId);
    }
  };

  const handleLoadMore = () => {
    loadMoreShelves();
  };

  const handleCreateShelf = async (name: string): Promise<void> => {
    loading = true;
    try {
      await api.createShelf(name);
      showMessage('Shelf created');
      await loadShelves();
    } catch {
      showMessage('Create shelf failed');
    } finally {
      loading = false;
    }
  };

  const handleRenameShelf = async (shelfId: string, name: string) => {
    loading = true;
    try {
      await api.renameShelf(shelfId, name);
      showMessage('Shelf renamed');
      await loadShelves();
    } catch {
      showMessage('Rename failed');
    } finally {
      loading = false;
    }
  };

  const handleDeleteShelf = async (shelfId: string) => {
    loading = true;
    try {
      await api.deleteShelf(shelfId);
      showMessage('Shelf deleted');
      updateUrlParams({ selected: null });
      await loadShelves();
    } catch {
      showMessage('Delete failed');
    } finally {
      loading = false;
    }
  };

  const handleOpenInLibrary = (book: Book) => {
    goto(`/library/books/${book.id}`);
  };

  const handleAddBook = async (shelfId: string, bookId: string) => {
    loading = true;
    try {
      await api.addToShelf(shelfId, [bookId]);
      showMessage('Book added to shelf');
      await loadShelfItems(shelfId);
      await loadShelves(); // Refresh counts
    } catch {
      showMessage('Failed to add book');
    } finally {
      loading = false;
    }
  };

  const handleRemoveBook = async (shelfId: string, bookId: string) => {
    loading = true;
    try {
      await api.removeFromShelf(shelfId, [bookId]);
      showMessage('Book removed from shelf');
      await loadShelfItems(shelfId);
      await loadShelves(); // Refresh counts
    } catch {
      showMessage('Failed to remove book');
    } finally {
      loading = false;
    }
  };
</script>

<svelte:head>
  <title>Shelves — Kobbler</title>
</svelte:head>

<ShelvesPage
  {shelves}
  {shelvesNextCursor}
  {shelvesTotal}
  {shelfItems}
  {loading}
  loadStateShelves={loadState.shelves}
  loadStateShelfItems={loadState.shelfItems}
  onSelectShelf={handleSelectShelf}
  onLoadMore={handleLoadMore}
  onCreateShelf={handleCreateShelf}
  onRenameShelf={handleRenameShelf}
  onDeleteShelf={handleDeleteShelf}
  onOpenInLibrary={handleOpenInLibrary}
  onAddBook={handleAddBook}
  onRemoveBook={handleRemoveBook}
/>
