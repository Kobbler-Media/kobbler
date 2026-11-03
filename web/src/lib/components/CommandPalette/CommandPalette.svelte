<script lang="ts">
  import { goto } from '$app/navigation';
  import {
    getUIState,
    closeCommandPalette,
    setCommandPaletteQuery,
    setCommandPaletteIndex,
    toggleTheme,
    registerOverlay,
    unregisterOverlay
  } from '#lib/stores/ui.svelte.js';
  import {
    unifiedSearch,
    triggerReindex,
    reindexTranscripts,
    clearMetadataCache,
    triggerLibraryCleanup,
    triggerMetadataWriteAll,
    triggerSidecarRewrite,
    clearProxyLogs,
    deleteCompletedJobs,
    clearRecents
  } from '#lib/api/client.js';
  import type { GroupedResults, SearchResult } from './types';
  import { search, flattenResults, getResultUrl, parseQuery } from './search';
  import { recordAccess, getRecentResults } from './recency';
  import { commands as commandRegistry } from './registry';
  import CommandResults from './CommandResults.svelte';
  import { useSearchSuggestions } from '#lib/composables/useSearchSuggestions.svelte.js';
  import { ghostText } from '#lib/actions/ghostText.js';
  import ChatResults from './ChatResults.svelte';
  import {
    getChatState,
    sendMessage,
    loadChatStatus,
    clearChat,
    pokeInactivityTimer,
    pauseInactivityTimer
  } from '#lib/stores/chat.svelte.js';

  const ui = getUIState();
  const chat = getChatState();
  const suggestions = useSearchSuggestions({ onPick: acceptSuggestion });

  let chatStatusInit = false;
  $effect(() => {
    if (ui.commandPaletteOpen && !chatStatusInit) {
      chatStatusInit = true;
      loadChatStatus();
    }
  });

  let inputRef: HTMLInputElement | undefined = $state();
  let chatMode = $state(false);
  let results = $state<GroupedResults>({
    topHits: [],
    books: [],
    ingest: [],
    authors: [],
    series: [],
    movies: [],
    tv_shows: [],
    tv_seasons: [],
    tv_episodes: [],
    pages: [],
    commands: [],
    settings: [],
    sections: [],
    externalSearch: [],
    transcripts: [],
    iptv_channels: [],
    livetv_channels: []
  });
  let loading = $state(false);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  // Focus input when palette opens, reset chat mode and clear chat when it closes
  $effect(() => {
    if (ui.commandPaletteOpen && inputRef) {
      inputRef.focus();
    } else {
      chatMode = false;
      clearChat();
      suggestions.close();
    }
  });

  // Register/unregister overlay for back-button behavior
  $effect(() => {
    if (ui.commandPaletteOpen) {
      registerOverlay('command-palette', closeCommandPalette);
      return () => unregisterOverlay('command-palette');
    }
  });

  // Search when query changes
  $effect(() => {
    const query = ui.commandPaletteQuery;

    // Clear previous timer
    if (debounceTimer) {
      clearTimeout(debounceTimer);
    }

    // Debounce API calls
    debounceTimer = setTimeout(async () => {
      // Don't search when in chat mode
      if (chatMode) {
        loading = false;
        return;
      }

      if (!query.trim()) {
        // Show recent items when query is empty - as a flat frecency-sorted list
        const recent = await getRecentResults();
        results = {
          topHits: recent, // Frecency-sorted flat list goes in topHits
          books: [],
          ingest: [],
          authors: [],
          series: [],
          movies: [],
          tv_shows: [],
          tv_seasons: [],
          tv_episodes: [],
          pages: [],
          commands: [],
          settings: [],
          sections: [],
          externalSearch: [],
          transcripts: [],
          iptv_channels: [],
          livetv_channels: []
        };
        return;
      }

      loading = true;
      try {
        results = await search(query, unifiedSearch);
      } finally {
        loading = false;
      }
    }, 150);
  });

  // Get section boundaries for arrow navigation
  function getSectionBoundaries(
    flat: SearchResult[]
  ): { start: number; end: number; type: string }[] {
    const boundaries: { start: number; end: number; type: string }[] = [];
    let currentType: string | null = null;
    let sectionStart = 0;

    for (let i = 0; i < flat.length; i++) {
      const type = flat[i].type;
      if (type !== currentType) {
        if (currentType !== null) {
          boundaries.push({ start: sectionStart, end: i - 1, type: currentType });
        }
        currentType = type;
        sectionStart = i;
      }
    }
    if (currentType !== null) {
      boundaries.push({ start: sectionStart, end: flat.length - 1, type: currentType });
    }
    return boundaries;
  }

  function handleKeydown(e: KeyboardEvent) {
    const flat = flattenResults(results);
    const count = flat.length;

    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        if (count > 0) {
          setCommandPaletteIndex((ui.commandPaletteIndex + 1) % count);
        }
        break;

      case 'ArrowUp':
        e.preventDefault();
        if (count > 0) {
          setCommandPaletteIndex((ui.commandPaletteIndex - 1 + count) % count);
        }
        break;

      case ']':
        if (count > 0) {
          e.preventDefault();
          const boundariesNext = getSectionBoundaries(flat);
          const currentIdxNext = ui.commandPaletteIndex;
          // Find current section and jump to next
          for (let i = 0; i < boundariesNext.length; i++) {
            if (
              currentIdxNext >= boundariesNext[i].start &&
              currentIdxNext <= boundariesNext[i].end
            ) {
              // Jump to next section (or wrap to first)
              const nextSection = boundariesNext[(i + 1) % boundariesNext.length];
              setCommandPaletteIndex(nextSection.start);
              break;
            }
          }
        }
        break;

      case '[':
        if (count > 0) {
          e.preventDefault();
          const boundariesPrev = getSectionBoundaries(flat);
          const currentIdxPrev = ui.commandPaletteIndex;
          // Find current section and jump to previous
          for (let i = 0; i < boundariesPrev.length; i++) {
            if (
              currentIdxPrev >= boundariesPrev[i].start &&
              currentIdxPrev <= boundariesPrev[i].end
            ) {
              // Jump to previous section (or wrap to last)
              const prevSection =
                boundariesPrev[(i - 1 + boundariesPrev.length) % boundariesPrev.length];
              setCommandPaletteIndex(prevSection.start);
              break;
            }
          }
        }
        break;

      case 'Enter':
        e.preventDefault();
        if (mode === 'chat') {
          const chatQuery = ui.commandPaletteQuery.trim();
          if (chatQuery) {
            sendMessage(chatQuery);
            setCommandPaletteQuery('');
          }
          break;
        }
        if (count > 0 && ui.commandPaletteIndex < count) {
          selectResult(flat[ui.commandPaletteIndex]);
        }
        break;

      case 'Escape':
        e.preventDefault();
        closeCommandPalette();
        break;
    }
  }

  function suggest(value: string) {
    if (chatMode || parseQuery(value).mode !== 'all') suggestions.close();
    else suggestions.input(value, ['movies', 'tv', 'books']);
  }

  function acceptSuggestion(value: string) {
    setCommandPaletteQuery(value);
    suggest(value);
  }

  function selectResult(result: SearchResult) {
    // Handle external search — navigate to Discover page
    if (result.type === 'external_search') {
      const { query } = parseQuery(ui.commandPaletteQuery);
      closeCommandPalette();
      goto(`/discover?q=${encodeURIComponent(query)}`);
      return;
    }

    // Execute command or navigate
    if (result.type === 'command' && result.action) {
      // Check if command needs confirmation
      const cmd = commandRegistry.find((c) => c.action === result.action);
      if (cmd?.confirmMessage && !confirm(cmd.confirmMessage)) {
        return;
      }

      // Record for frecency only after confirmation
      recordAccess(result);
      executeCommand(result.action);
      closeCommandPalette();
      return;
    }

    // Record for frecency
    recordAccess(result);

    const url = getResultUrl(result);
    if (url) {
      goto(url);
      closeCommandPalette();
    }
  }

  async function executeCommand(action: string) {
    switch (action) {
      case 'toggleTheme':
        toggleTheme();
        break;

      case 'refreshLibrary':
        window.location.reload();
        break;

      case 'reindexSearch':
        try {
          await triggerReindex();
        } catch (e) {
          console.error('Reindex failed:', e);
        }
        break;

      case 'reindexTranscripts':
        try {
          await reindexTranscripts();
        } catch (e) {
          console.error('Transcript reindex failed:', e);
        }
        break;

      case 'clearMetadataCache':
        try {
          await clearMetadataCache();
        } catch (e) {
          console.error('Clear metadata cache failed:', e);
        }
        break;

      case 'libraryCleanup':
        try {
          await triggerLibraryCleanup();
        } catch (e) {
          console.error('Library cleanup failed:', e);
        }
        break;

      case 'writeMetadataAll':
        try {
          await triggerMetadataWriteAll();
        } catch (e) {
          console.error('Write metadata failed:', e);
        }
        break;

      case 'sidecarRewrite':
        try {
          await triggerSidecarRewrite();
        } catch (e) {
          console.error('Sidecar rewrite failed:', e);
        }
        break;

      case 'clearProxyLogs':
        try {
          await clearProxyLogs();
        } catch (e) {
          console.error('Clear proxy logs failed:', e);
        }
        break;

      case 'clearCompletedJobs':
        try {
          await deleteCompletedJobs();
        } catch (e) {
          console.error('Clear completed jobs failed:', e);
        }
        break;

      case 'clearRecentSearches':
        try {
          await clearRecents();
        } catch (e) {
          console.error('Clear recent searches failed:', e);
        }
        break;
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      closeCommandPalette();
    }
  }

  function handleInput(e: Event) {
    const target = e.target as HTMLInputElement;
    const val = target.value;

    // Toggle chat mode when ? is typed as sole character
    if (val === '?' && !chatMode && chat.chatStatus.configured) {
      chatMode = true;
      setCommandPaletteQuery('');
      target.value = '';
      return;
    }
    // Exit chat mode if ? is typed while already in chat mode with empty input
    if (val === '?' && chatMode && !ui.commandPaletteQuery) {
      chatMode = false;
      clearChat();
      setCommandPaletteQuery('');
      target.value = '';
      return;
    }

    setCommandPaletteQuery(val);
    suggest(val);

    // Manage inactivity timer: pause while typing, restart when input cleared
    if (chatMode) {
      if (val.trim()) {
        pauseInactivityTimer();
      } else {
        pokeInactivityTimer();
      }
    }
  }

  // Get placeholder based on OS
  const isMac =
    typeof navigator !== 'undefined' &&
    (navigator.platform?.toLowerCase().includes('mac') ||
      (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform
        ?.toLowerCase()
        .includes('mac'));

  const placeholder = isMac ? 'Search... \u2318K' : 'Search... Ctrl+K';
  const { mode } = $derived(
    chatMode ? { mode: 'chat' as const } : parseQuery(ui.commandPaletteQuery)
  );

  function getModeHint(): string | null {
    switch (mode) {
      case 'commands':
        return 'Searching commands';
      case 'authors':
        return 'Searching authors';
      case 'series':
        return 'Searching series';
      case 'pages':
        return 'Searching pages';
      case 'books':
        return 'Searching books';
      case 'transcripts':
        return 'Searching transcripts';
      case 'chat':
        return 'Chat mode';
      default:
        return null;
    }
  }

  // Check if we should show prefix hints (when query is short and no mode selected)
  const showPrefixHints = $derived(
    mode === 'all' && ui.commandPaletteQuery.length < 2 && !chatMode
  );
  const chatPlaceholder = 'Ask about your library...';
</script>

{#if ui.commandPaletteOpen}
  <div
    class="command-palette-backdrop"
    role="presentation"
    onclick={handleBackdropClick}
    onkeydown={(e) => e.key === 'Escape' && closeCommandPalette()}
  >
    <div class="command-palette" role="dialog" aria-modal="true" aria-label="Command palette">
      <div class="search-input-container">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" class="search-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          bind:this={inputRef}
          type="text"
          class="search-input"
          placeholder={chatMode ? chatPlaceholder : placeholder}
          value={ui.commandPaletteQuery}
          oninput={handleInput}
          onkeydown={handleKeydown}
          use:ghostText={{
            suggestion: suggestions.open ? suggestions.items[0]?.title : undefined,
            onaccept: acceptSuggestion
          }}
          autocomplete="off"
          spellcheck="false"
        />
        {#if loading}
          <div class="loading-spinner"></div>
        {/if}
      </div>

      {#if getModeHint()}
        <div class="mode-hint">{getModeHint()}</div>
      {/if}

      <div class="divider"></div>

      {#if mode === 'chat' && chat.chatStatus.configured}
        <ChatResults />
      {:else}
        <CommandResults
          {results}
          selectedIndex={ui.commandPaletteIndex}
          onSelect={selectResult}
          isEmptyQuery={!ui.commandPaletteQuery.trim()}
        />
      {/if}

      <div class="footer">
        {#if showPrefixHints}
          <div class="prefix-hints">
            <span><kbd>#</kbd> books</span>
            <span><kbd>@</kbd> authors</span>
            <span><kbd>:</kbd> series</span>
            <span><kbd>/</kbd> pages</span>
            <span><kbd>&gt;</kbd> commands</span>
            <span><kbd>~</kbd> transcripts</span>
            {#if chat.chatStatus.configured}
              <span><kbd>?</kbd> chat</span>
            {/if}
          </div>
        {:else if mode === 'chat'}
          <div class="footer-hints">
            <span><kbd>↵</kbd> Send</span>
            <span><kbd>?</kbd> Exit chat</span>
            <span><kbd>esc</kbd> Close</span>
          </div>
        {:else}
          <div class="footer-hints">
            <span><kbd>↑↓</kbd> Navigate</span>
            <span><kbd>[]</kbd> Sections</span>
            <span><kbd>↵</kbd> Select</span>
            <span><kbd>esc</kbd> Close</span>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .command-palette-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 15vh;
    z-index: 1000;
    backdrop-filter: blur(2px);
  }

  .command-palette {
    width: 100%;
    max-width: 580px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    box-shadow:
      0 20px 40px rgba(0, 0, 0, 0.4),
      0 0 0 1px rgba(255, 255, 255, 0.05);
    overflow: hidden;
  }

  :global([data-theme='light']) .command-palette {
    box-shadow:
      0 20px 40px rgba(0, 0, 0, 0.15),
      0 0 0 1px rgba(0, 0, 0, 0.05);
  }

  .search-input-container {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px;
  }

  .search-icon {
    flex-shrink: 0;
    color: var(--muted);
  }

  .search-input {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--ink);
    font-size: 16px;
    outline: none;
  }

  .search-input::placeholder {
    color: var(--muted);
  }

  .loading-spinner {
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

  .mode-hint {
    padding: 0 16px 8px;
    font-size: 12px;
    color: var(--accent);
  }

  .divider {
    height: 1px;
    background: var(--ring);
  }

  .footer {
    padding: 8px 16px;
    border-top: 1px solid var(--ring);
    background: rgba(0, 0, 0, 0.1);
  }

  :global([data-theme='light']) .footer {
    background: rgba(0, 0, 0, 0.03);
  }

  .footer-hints {
    display: flex;
    gap: 16px;
    font-size: 11px;
    color: var(--muted);
  }

  .footer-hints kbd {
    display: inline-block;
    padding: 2px 5px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    font-family: inherit;
    font-size: 10px;
    margin-right: 4px;
  }

  :global([data-theme='light']) .footer-hints kbd {
    background: rgba(0, 0, 0, 0.08);
  }

  .prefix-hints {
    display: flex;
    gap: 12px;
    font-size: 11px;
    color: var(--muted);
    flex-wrap: wrap;
  }

  .prefix-hints kbd {
    display: inline-block;
    padding: 2px 6px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    font-family: monospace;
    font-size: 11px;
    margin-right: 2px;
    color: var(--accent);
  }

  :global([data-theme='light']) .prefix-hints kbd {
    background: rgba(0, 0, 0, 0.08);
  }

  @media (max-width: 640px) {
    .command-palette-backdrop {
      padding: 8px;
      padding-top: 10vh;
    }

    .command-palette {
      max-width: none;
    }
  }
</style>
