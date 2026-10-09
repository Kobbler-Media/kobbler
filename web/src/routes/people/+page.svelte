<script lang="ts">
  import { goto } from '$app/navigation';
  import { showMessage } from '$lib/context/app.svelte';
  import PersonCard from '$lib/components/shared/PersonCard.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import BidirectionalScroll from '$lib/components/BidirectionalScroll.svelte';
  import * as api from '$lib/api/client';

  let people = $state<api.PersonListItem[]>([]);
  let nextCursor = $state<string | null>(null);
  let prevCursor = $state<string | null>(null);
  let totalCount = $state(0);
  let loading = $state(false);
  let error = $state('');
  let searchQuery = $state('');
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  let showCreateForm = $state(false);
  let newName = $state('');
  let creating = $state(false);

  // Parse search_offset from a cursor like "search_offset:50"
  function parseSearchOffset(cursor: string): number | undefined {
    if (cursor.startsWith('search_offset:')) {
      return parseInt(cursor.slice('search_offset:'.length), 10);
    }
    return undefined;
  }

  async function loadPeople(opts: { cursor?: string; direction?: 'forward' | 'backward' } = {}) {
    loading = true;
    try {
      const searchOffset = opts.cursor ? parseSearchOffset(opts.cursor) : undefined;
      const resp = await api.listPeople({
        q: searchQuery.trim() || undefined,
        cursor: searchOffset !== undefined ? undefined : opts.cursor,
        direction: opts.direction,
        search_offset: searchOffset
      });
      if (opts.direction === 'backward') {
        const existingIds = new Set(people.map((p) => p.id));
        const uniqueNew = resp.items.filter((p) => !existingIds.has(p.id));
        people = [...uniqueNew, ...people];
      } else if (opts.cursor) {
        const existingIds = new Set(people.map((p) => p.id));
        const uniqueNew = resp.items.filter((p) => !existingIds.has(p.id));
        people = [...people, ...uniqueNew];
      } else {
        people = resp.items;
      }
      nextCursor = resp.next_cursor ?? null;
      prevCursor = resp.prev_cursor ?? null;
      if (resp.total_count !== undefined) {
        totalCount = resp.total_count;
      }
      error = '';
    } catch (e) {
      showMessage('Failed to load people');
      console.error('Failed to load people:', e);
      error = 'Failed to load data. Please check your connection and try again.';
    } finally {
      loading = false;
    }
  }

  function loadMore() {
    if (nextCursor && !loading) {
      loadPeople({ cursor: nextCursor, direction: 'forward' });
    }
  }

  function loadPrevious() {
    if (prevCursor && !loading) {
      loadPeople({ cursor: prevCursor, direction: 'backward' });
    }
  }

  function handleSearchInput(e: Event) {
    const target = e.target as HTMLInputElement;
    searchQuery = target.value;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      loadPeople();
    }, 200);
  }

  async function handleCreate() {
    const trimmed = newName.trim();
    if (!trimmed) return;
    creating = true;
    try {
      const result = await api.createPerson({ name: trimmed });
      showMessage(`Created ${trimmed}`);
      newName = '';
      showCreateForm = false;
      await loadPeople();
      goto(`/people/${result.id}`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create person');
    } finally {
      creating = false;
    }
  }

  function handleCreateKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      handleCreate();
    } else if (e.key === 'Escape') {
      showCreateForm = false;
      newName = '';
    }
  }

  $effect(() => {
    loadPeople();
  });
</script>

<svelte:head>
  <title>People — Kobbler</title>
</svelte:head>

<div class="page page-container">
  <div class="page-header">
    <div class="header-left">
      <h1>People</h1>
      {#if totalCount > 0}
        <span class="count">{totalCount}</span>
      {/if}
    </div>
    <div class="header-actions">
      <input
        class="search-input"
        type="text"
        placeholder="Search people..."
        value={searchQuery}
        oninput={handleSearchInput}
      />
      <button class="btn-primary" onclick={() => (showCreateForm = !showCreateForm)}>
        <span class="material-icons-outlined">person_add</span>
        Add Person
      </button>
    </div>
  </div>

  {#if showCreateForm}
    <div class="create-form">
      <input
        class="create-input"
        type="text"
        placeholder="Person name..."
        bind:value={newName}
        onkeydown={handleCreateKeydown}
      />
      <button class="btn-primary" onclick={handleCreate} disabled={!newName.trim() || creating}>
        {creating ? 'Creating...' : 'Create'}
      </button>
      <button
        class="btn-sec"
        onclick={() => {
          showCreateForm = false;
          newName = '';
        }}
      >
        Cancel
      </button>
    </div>
  {/if}

  {#if error}
    <ErrorBanner message={error} onRetry={() => loadPeople()} onDismiss={() => (error = '')} />
  {/if}

  {#if loading && people.length === 0}
    <div class="loading">Loading...</div>
  {:else if people.length === 0}
    <div class="empty">
      {#if searchQuery.trim()}
        No people found matching "{searchQuery}"
      {:else}
        No people yet. Add one to get started.
      {/if}
    </div>
  {:else}
    <BidirectionalScroll
      hasMore={nextCursor !== null}
      hasPrevious={prevCursor !== null}
      {loading}
      onLoadMore={loadMore}
      onLoadPrevious={loadPrevious}
    >
      <div class="people-grid">
        {#each people as person (person.id)}
          <button
            class="person-card-btn"
            type="button"
            data-person-id={person.id}
            onclick={() => goto(`/people/${person.id}`)}
          >
            <PersonCard
              name={person.name}
              role={null}
              imageUrl={person.image_url ? api.personImageUrl(person.id) : null}
            />
          </button>
        {/each}
      </div>
    </BidirectionalScroll>
  {/if}
</div>

<style>
  .page {
    padding-top: 24px;
    padding-bottom: 80px;
  }

  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
    gap: 16px;
    flex-wrap: wrap;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .page-header h1 {
    font-size: 22px;
    font-weight: 700;
    color: var(--ink);
    margin: 0;
  }

  .count {
    font-size: 13px;
    color: var(--muted);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .search-input {
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--bg);
    color: var(--ink);
    font-size: 13px;
    width: 220px;
  }

  .search-input::placeholder {
    color: var(--muted);
  }

  .create-form {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 20px;
    padding: 12px 16px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .create-input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--bg);
    color: var(--ink);
    font-size: 13px;
  }

  .create-input::placeholder {
    color: var(--muted);
  }

  .people-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: 10px;
  }

  .person-card-btn {
    display: block;
    width: 100%;
    padding: 12px 16px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s;
  }

  .person-card-btn:hover {
    border-color: var(--accent);
  }

  .loading,
  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 60px 20px;
    color: var(--muted);
    font-size: 14px;
  }

  @media (max-width: 768px) {
    .header-actions {
      flex-wrap: wrap;
      width: 100%;
    }

    .search-input {
      width: 100%;
    }

    .create-form {
      flex-wrap: wrap;
    }

    .people-grid {
      grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
      gap: 8px;
    }
  }
</style>
