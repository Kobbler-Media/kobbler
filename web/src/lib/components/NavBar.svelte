<script lang="ts">
  import JobsIndicator from './JobsIndicator.svelte';
  import NotificationsIndicator from './NotificationsIndicator.svelte';
  import { openCommandPalette } from '$lib/stores/ui.svelte';

  interface Props {
    isAuthed: boolean;
    onToggleMenu: () => void;
  }

  let { isAuthed, onToggleMenu }: Props = $props();

  // Detect OS for keyboard shortcut hint
  const isMac =
    typeof navigator !== 'undefined' &&
    (navigator.platform?.toLowerCase().includes('mac') ||
      (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform
        ?.toLowerCase()
        .includes('mac'));

  const searchPlaceholder = isMac ? 'Search... \u2318K' : 'Search... Ctrl+K';

  function handleSearchFocus() {
    openCommandPalette();
  }
</script>

<header class="topbar">
  {#if isAuthed}
    <button class="mobile-menu" type="button" aria-label="Menu" onclick={onToggleMenu}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
        <path d="M4 7h16v2H4V7zm0 5h16v2H4v-2zm0 5h16v2H4v-2z"></path>
      </svg>
    </button>
  {/if}
  <a href="/" class="brand">Kobbler</a>

  {#if isAuthed}
    <button class="search-trigger" type="button" onclick={handleSearchFocus}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
        <path
          d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
        ></path>
      </svg>
      <span class="search-placeholder">{searchPlaceholder}</span>
    </button>
  {/if}

  <div class="topbar-actions">
    {#if isAuthed}
      <button
        class="icon-btn mobile-search"
        type="button"
        aria-label="Search"
        onclick={handleSearchFocus}
      >
        <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
      </button>
      <JobsIndicator />
      <NotificationsIndicator />
    {/if}
  </div>
</header>

<style>
  .topbar {
    position: sticky;
    top: 0;
    z-index: 50;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: var(--safe-top) 20px 0;
    height: calc(56px + var(--safe-top));
    border-bottom: 1px solid var(--ring);
    background: linear-gradient(180deg, rgba(21, 24, 30, 0.95), rgba(17, 20, 25, 0.85));
    backdrop-filter: blur(6px);
    box-sizing: border-box;
  }

  :global([data-theme='light']) .topbar {
    background: linear-gradient(180deg, rgba(245, 239, 231, 0.95), rgba(239, 230, 219, 0.85));
  }

  .brand {
    font-family: inherit;
    font-size: 24px;
    font-weight: 700;
    letter-spacing: 0.6px;
    white-space: nowrap;
    text-decoration: none;
    color: var(--ink);
    line-height: 32px;
  }

  .brand:hover {
    color: var(--accent);
  }

  .topbar-actions {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .search-trigger {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 14px;
    height: 32px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    color: var(--muted);
    font-size: 13px;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background 0.15s;
    flex: 1;
    min-width: 180px;
    max-width: 500px;
  }

  .search-trigger:hover {
    border-color: var(--accent);
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .search-trigger:hover {
    background: rgba(0, 0, 0, 0.03);
  }

  .search-icon {
    flex-shrink: 0;
    opacity: 0.7;
  }

  .search-placeholder {
    flex: 1;
    text-align: left;
  }

  .icon-btn {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    width: 32px;
    height: 32px;
    cursor: pointer;
    color: var(--ink);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }

  .icon-btn:hover {
    background: var(--ring);
  }

  .mobile-menu {
    display: none;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    border-radius: 12px;
    width: 32px;
    height: 32px;
    cursor: pointer;
    align-items: center;
    justify-content: center;
    padding: 0;
  }

  .mobile-search {
    display: none;
  }

  @media (max-width: 768px) {
    .topbar {
      padding: var(--safe-top) 12px 0;
      height: calc(48px + var(--safe-top));
      justify-content: flex-start;
    }

    .brand {
      font-size: 20px;
      line-height: 28px;
    }

    .topbar-actions {
      margin-left: auto;
    }

    .mobile-menu {
      display: inline-flex;
    }

    .mobile-search {
      display: inline-flex;
    }

    .search-trigger {
      display: none;
    }
  }

  @media (min-width: 769px) and (max-width: 1024px) {
    .search-trigger {
      min-width: 140px;
    }
  }
</style>
