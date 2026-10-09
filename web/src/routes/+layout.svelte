<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import '$lib/styles/global.css';
  import '$lib/styles/panel.css';
  import { goto, beforeNavigate } from '$app/navigation';
  import favicon from '$lib/assets/favicon.svg';
  import { Toast } from '@koblibs/ui';
  import { ScrollToTop } from '@koblibs/ui';
  import LoginForm from '$lib/components/LoginForm.svelte';
  import NavBar from '$lib/components/NavBar.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import MobileMenu from '$lib/components/MobileMenu.svelte';
  import { CommandPalette } from '$lib/components/CommandPalette';
  import MiniPlayer from '$lib/components/AudiobookPlayer/MiniPlayer.svelte';
  import CastMiniController from '$lib/components/CastMiniController.svelte';
  import {
    getStreamState,
    setApiBase,
    on,
    onDebounced,
    setOnReconnect,
    ensureStream,
    retryStream,
    cleanup as cleanupStream
  } from '$lib/services/eventstream.svelte';
  import { initCast } from '$lib/stores/cast.svelte';
  import { handleKobloadDownloadEvent } from '$lib/stores/downloads.svelte';
  import { handleJobEvent, loadJobs, syncRunningCount } from '$lib/stores/jobs.svelte';
  import {
    handleNotificationCreated,
    loadNotifications,
    loadUnreadCount
  } from '$lib/stores/notifications.svelte';
  import { updateWantStatus } from '$lib/stores/wantStatus.svelte';
  import { fetchStats } from '$lib/stores/stats.svelte';
  import { getAuthState, setAuthed, setRole, setEmail } from '$lib/stores/auth.svelte';
  import {
    getUIState,
    setTheme,
    setMenuOpen,
    openCommandPalette,
    hasOpenOverlays,
    closeTopOverlay
  } from '$lib/stores/ui.svelte';
  import { checkAuth, loadLibraryDevices } from '$lib/services/data.svelte';
  import { loadLibrary, refreshBook, ensureDevices } from '$lib/context/app.svelte';

  let { children } = $props();

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  // Store state
  const auth = getAuthState();
  const ui = getUIState();

  // Local state
  let loading = $state(false);
  let hydrated = $state(false);
  let actionMessage = $state('');

  // Stream state
  const streamState = getStreamState();
  const streamGaveUp = $derived(streamState.gaveUp);

  // Derived from stores
  const isAuthed = $derived(auth.isAuthed);
  const theme = $derived(ui.theme);
  const menuOpen = $derived(ui.menuOpen);

  const handleLogin = async (email: string, password: string) => {
    loading = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, password }),
        credentials: 'include'
      });
      if (!resp.ok) {
        let message = 'Login failed';
        try {
          const error = await resp.json();
          if (typeof error?.message === 'string' && error.message.trim()) {
            message = error.message.trim();
          }
        } catch {
          // Leave default message
        }
        throw new Error(message);
      }
      const data = await resp.json();
      setAuthed(true);
      setRole(data.role || 'viewer');
      setEmail(data.email || '');
      ensureDevices();
      await loadLibraryDevices();
      ensureStream(true);

      // Navigate to library or restore last page
      const lastPage = localStorage.getItem('kobbler-page') || 'library';
      if ($page.url.pathname === '/') {
        goto(`/${lastPage}`, { replaceState: true });
      }
    } finally {
      loading = false;
    }
  };

  // Save current page to localStorage when it changes
  $effect(() => {
    const pathname = $page.url.pathname;
    const pageName = pathname.slice(1).split('/')[0];
    if (pageName && isAuthed) {
      localStorage.setItem('kobbler-page', pageName);
    }
  });

  // Global keyboard shortcut for command palette
  function handleKeydown(e: KeyboardEvent) {
    // Cmd+K (Mac) or Ctrl+K (Windows/Linux)
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
      e.preventDefault();
      if (isAuthed) {
        openCommandPalette();
      }
    }
  }

  beforeNavigate(({ type, cancel }) => {
    if (type === 'popstate' && hasOpenOverlays()) {
      cancel();
      closeTopOverlay();
    }
  });

  onDestroy(() => {
    cleanupStream();
  });

  onMount(async () => {
    setApiBase(apiBase);

    // Register event callbacks for the unified SSE stream
    on('library_updated', (data) => {
      if (data?.book_id) {
        refreshBook(data.book_id);
      }
    });
    onDebounced('library_updated', () => fetchStats(true));
    onDebounced('tv_show_updated', () => fetchStats(true));
    onDebounced('movie_updated', () => fetchStats(true));
    onDebounced('author_updated', () => fetchStats(true));
    onDebounced('series_updated', () => fetchStats(true));
    on('kobload_download_snapshot', (data) => handleKobloadDownloadEvent(data));
    on('kobload_download_created', (data) => handleKobloadDownloadEvent(data));
    on('kobload_download_updated', (data) => handleKobloadDownloadEvent(data));
    on('wanted_status_updated', (data) => updateWantStatus(data.wanted_id, data.status ?? ''));
    on('job', (data) => handleJobEvent(data));
    on('jobs_resync', () => syncRunningCount());
    on('notification_created', () => handleNotificationCreated());

    // When reconnecting after stale period, refresh all data
    setOnReconnect(() => {
      fetchStats(true);
      loadLibrary();
      loadJobs(10);
      syncRunningCount();
      loadNotifications(10);
      loadUnreadCount();
    });

    // Initialize theme
    const storedTheme = localStorage.getItem('kobbler-theme');
    if (storedTheme === 'light' || storedTheme === 'dark') {
      setTheme(storedTheme);
    } else if (window.matchMedia?.('(prefers-color-scheme: light)').matches) {
      setTheme('light');
    } else {
      setTheme('dark');
    }

    // Cleanup startup meta tag to avoid duplication/conflicts
    const startupMeta = document.getElementById('startup-theme-color');
    if (startupMeta) startupMeta.remove();

    // Check if already authenticated with a lightweight probe
    try {
      const authed = await checkAuth();
      if (authed) {
        ensureDevices();
        await loadLibraryDevices();
        ensureStream(true);

        // Initialize Google Cast SDK if a receiver app ID is configured
        if (auth.castReceiverAppId) {
          initCast(auth.castReceiverAppId);
        }
      }
    } catch {
      // Not authenticated, will show login form
    }

    // Dev-only: warn when main thread is blocked >200ms
    if (import.meta.env.DEV && typeof PerformanceObserver !== 'undefined') {
      const observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (entry.duration > 200) {
            console.warn(`[Long task] ${Math.round(entry.duration)}ms`, entry);
          }
        }
      });
      observer.observe({ type: 'longtask', buffered: true });
    }

    // Register service worker for web push notifications
    if ('serviceWorker' in navigator) {
      navigator.serviceWorker.register('/sw.js').catch((err) => {
        console.warn('Service worker registration failed:', err);
      });
    }

    hydrated = true;
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<svelte:head>
  <title>Kobbler</title>
  <link rel="icon" href={favicon} />
  <link rel="manifest" href="/manifest.json" />
  <meta name="mobile-web-app-capable" content="yes" />
  <meta name="apple-mobile-web-app-status-bar-style" content="black-translucent" />
  <meta name="color-scheme" content="dark light" />
  <meta name="theme-color" content={theme === 'dark' ? '#0f1115' : '#f5efe7'} />
</svelte:head>

<div class="app" data-theme={theme}>
  <NavBar {isAuthed} onToggleMenu={() => setMenuOpen(!menuOpen)} />

  {#if streamGaveUp && isAuthed}
    <div class="connection-banner">
      <span>Live updates unavailable — reconnecting…</span>
      <button type="button" onclick={() => retryStream(true)}>Retry</button>
    </div>
  {/if}

  <div class="app-body">
    {#if isAuthed}
      <Sidebar />
    {/if}

    <main class="content">
      {#if !hydrated}
        <div class="loading-card">Loading…</div>
      {:else if !isAuthed}
        <LoginForm onLogin={handleLogin} {loading} />
      {:else}
        <Toast message={actionMessage} />
        {@render children()}
        <ScrollToTop />
      {/if}
    </main>
  </div>
</div>

<MobileMenu open={menuOpen} onClose={() => setMenuOpen(false)} />

{#if isAuthed}
  <CommandPalette />
  <MiniPlayer />
  <CastMiniController />
{/if}

<style>
  :global(:root) {
    --safe-top: env(safe-area-inset-top, 0px);
    --safe-right: env(safe-area-inset-right, 0px);
    --safe-bottom: env(safe-area-inset-bottom, 0px);
    --safe-left: env(safe-area-inset-left, 0px);
    --bg: #0f1115;
    --bg-2: #15181e;
    --ink: #f3f1ee;
    --muted: #9da6b0;
    --accent: #d38c5a;
    --accent-dim: rgba(211, 140, 90, 0.15);
    --accent-2: #2d6b67;
    --card: #191d24;
    --ring: #2a3038;
    --shadow: 0 18px 40px rgba(5, 7, 10, 0.5);
    --radius: 18px;
    --tag-bg: rgba(255, 255, 255, 0.08);
    --tag-text: #c6d2da;
    --danger: #ef4444;
    --success: #22c55e;
    --warning: #f97316;
    --wanted: #eab308;
    --monitored: #8b5cf6;
    --skipped: #6b7280;
    --panel-width: 420px;
  }

  :global([data-theme='light']) {
    --bg: #f5efe7;
    --bg-2: #efe6db;
    --ink: #1f1b17;
    --muted: #5f6b72;
    --accent: #c87a46;
    --accent-dim: rgba(200, 122, 70, 0.15);
    --accent-2: #276f6a;
    --card: #fff9f1;
    --ring: #d6c5b2;
    --shadow: 0 18px 40px rgba(28, 24, 20, 0.12);
    --tag-bg: rgba(0, 0, 0, 0.06);
    --tag-text: #5c6369;
    --danger: #dc2626;
    --success: #16a34a;
    --warning: #ea580c;
    --wanted: #ca8a04;
    --monitored: #7c3aed;
    --skipped: #4b5563;
    --panel-width: 420px;
  }

  :global(*),
  :global(*::before),
  :global(*::after) {
    box-sizing: border-box;
  }

  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }

  :global(body) {
    font-family: 'Inter', 'Segoe UI', system-ui, sans-serif;
    background-color: var(--bg);
    background: radial-gradient(circle at top, var(--bg-2) 0%, var(--bg) 50%, var(--bg-2) 100%);
    color: var(--ink);
  }

  :global(*) {
    scrollbar-width: thin;
    scrollbar-color: var(--ring) transparent;
  }

  :global(*::-webkit-scrollbar) {
    width: 8px;
    height: 8px;
  }

  :global(*::-webkit-scrollbar-track) {
    background: transparent;
  }

  :global(*::-webkit-scrollbar-thumb) {
    background: var(--ring);
    border-radius: 4px;
  }

  :global(*::-webkit-scrollbar-thumb:hover) {
    background: var(--muted);
  }

  .app {
    width: 100%;
    height: 100vh;
    height: 100dvh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    /* viewport-fit=cover + the standalone PWA manifest put the viewport under
       Android's navigation bar and display cutouts. */
    padding-bottom: var(--safe-bottom);
    padding-left: var(--safe-left);
    padding-right: var(--safe-right);
  }

  .app-body {
    flex: 1;
    display: flex;
    min-height: 0;
    overflow: hidden;
  }

  .connection-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 8px 16px;
    background: var(--accent-2);
    color: var(--ink);
    font-size: 0.875rem;
  }

  .connection-banner button {
    padding: 4px 12px;
    background: rgba(255, 255, 255, 0.15);
    border: 1px solid rgba(255, 255, 255, 0.25);
    border-radius: 6px;
    color: inherit;
    font-size: 0.8125rem;
    cursor: pointer;
    transition: background 0.15s;
  }

  .connection-banner button:hover {
    background: rgba(255, 255, 255, 0.25);
  }

  .content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-gutter: stable;
    display: flex;
    flex-direction: column;
    position: relative;
  }

  :global(select option) {
    color: #12161d;
    background: #ffffff;
  }

  .loading-card {
    max-width: 320px;
    margin: 60px auto 0;
    padding: 16px;
    border-radius: var(--radius);
    border: 1px solid var(--ring);
    background: var(--card);
    text-align: center;
  }
</style>
