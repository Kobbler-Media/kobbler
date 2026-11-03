<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import type { IptvChannel, IptvSource, IptvEpgEntry } from '#lib/types.js';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import { usePlayback } from '#lib/composables/usePlayback.svelte.js';
  import EpgTimeline from '#lib/components/iptv/EpgTimeline.svelte';
  import RecordButton from '#lib/components/iptv/RecordButton.svelte';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';

  const channelId = $derived(page.params.id ?? '');
  const sourceIdParam = $derived(page.url.searchParams.get('source') ?? undefined);
  const catchupEpgId = $derived(page.url.searchParams.get('catchup') ?? undefined);

  let channel = $state<IptvChannel | null>(null);
  let sources = $state<IptvSource[]>([]);
  let programs = $state<IptvEpgEntry[]>([]);
  let loading = $state(true);
  let error = $state('');
  let isFavourite = $state(false);
  let epgCollapsed = $state(false);

  let currentProgram = $derived(
    programs.find((p) => {
      const now = Date.now();
      return new Date(p.start_time).getTime() <= now && new Date(p.stop_time).getTime() > now;
    })
  );

  const playback = usePlayback({
    mediaType: 'live',
    refreshWatchState: async () => {}
  });

  async function loadChannel() {
    loading = true;
    error = '';
    try {
      channel = await api.getIptvChannel(channelId);
      const [sourcesRes, epgRes, favs] = await Promise.all([
        api.getIptvChannelSources(channelId),
        api.getIptvChannelEpg(channelId),
        api.getIptvFavourites()
      ]);
      sources = sourcesRes;
      programs = epgRes;
      isFavourite = favs.some((f) => f.id === channelId);

      // Record watch history (fire-and-forget)
      api.recordIptvWatch(channelId).catch(() => {});

      if (catchupEpgId) {
        // Catchup playback — returns a session with the timeshift direct_url
        try {
          const session = await api.catchupIptv(channelId, catchupEpgId);
          playback.playSession(session);
        } catch (e) {
          console.error('Catchup failed, falling back to live:', e);
          showMessage('Catchup unavailable, playing live instead');
          playback.play(channelId, sourceIdParam);
        }
      } else {
        // Live playback (optionally with a specific source)
        playback.play(channelId, sourceIdParam);
      }
    } catch (e) {
      console.error('Failed to load channel:', e);
      error = 'Failed to load channel. It may not exist or the server is unreachable.';
    } finally {
      loading = false;
    }
  }

  async function toggleFavourite() {
    try {
      if (isFavourite) {
        await api.removeIptvFavourite(channelId);
        isFavourite = false;
        showMessage('Removed from favourites');
      } else {
        await api.addIptvFavourite(channelId);
        isFavourite = true;
        showMessage('Added to favourites');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update favourite');
    }
  }

  function healthColor(score: number): string {
    if (score >= 80) return 'var(--success, #22c55e)';
    if (score >= 50) return 'var(--warning, #f59e0b)';
    return 'var(--danger, #ef4444)';
  }

  function handleBack() {
    goto('/iptv');
  }

  onMount(() => {
    loadChannel();
  });

  // Re-load only on navigation to a different channel (not initial mount)
  $effect(() => {
    if (channelId && channel && channel.id !== channelId) {
      loadChannel();
    }
  });
</script>

<svelte:head>
  <title>{channel?.name ?? 'Channel'} — Live TV — Kobbler</title>
</svelte:head>

<div class="channel-page page-container">
  <!-- Back nav -->
  <div class="back-bar">
    <button class="back-btn" type="button" onclick={handleBack}>
      <span class="material-icons-outlined">arrow_back</span>
      <span>Live TV</span>
    </button>
  </div>

  {#if error}
    <ErrorBanner message={error} onRetry={loadChannel} onDismiss={() => (error = '')} />
  {/if}

  {#if loading && !channel}
    <div class="loading-state">
      <div class="spinner"></div>
      <span class="loading-text">Loading channel...</span>
    </div>
  {:else if channel}
    <!-- Player -->
    {#if playback.session && playback.capabilities}
      <VideoPlayer
        sessionResponse={playback.session}
        creationParams={{
          media_type: 'live',
          file_id: channelId,
          capabilities: playback.capabilities
        }}
        mediaId={channelId}
        onClose={() => {
          playback.close();
          goto('/iptv');
        }}
      />
    {:else if !error}
      <div class="player-section player-loading">
        <div class="spinner"></div>
        <span class="loading-text">Starting stream...</span>
      </div>
    {/if}

    <!-- Channel info bar -->
    <div class="info-bar">
      <div class="info-left">
        {#if channel.icon_url}
          <img src={channel.icon_url} alt="" class="channel-icon" />
        {:else}
          <div class="channel-icon-placeholder">
            <span class="material-icons-outlined">live_tv</span>
          </div>
        {/if}
        <div class="info-text">
          <h1>{channel.name}</h1>
          {#if channel.current_program}
            <span class="now-playing">{channel.current_program}</span>
          {/if}
          <div class="channel-meta">
            {#if channel.category}
              <span class="meta-tag">{channel.category}</span>
            {/if}
            {#if channel.language}
              <span class="meta-tag">{channel.language}</span>
            {/if}
          </div>
        </div>
      </div>
      <div class="info-actions">
        <button
          class="action-btn"
          class:active={isFavourite}
          type="button"
          aria-label={isFavourite ? 'Remove from favourites' : 'Add to favourites'}
          onclick={toggleFavourite}
        >
          <span class="material-icons-outlined">
            {isFavourite ? 'favorite' : 'favorite_border'}
          </span>
          <span>{isFavourite ? 'Favourited' : 'Favourite'}</span>
        </button>
        <RecordButton
          {channelId}
          title={currentProgram?.title ?? channel.current_program ?? channel.name}
          epgId={currentProgram?.id}
        />
      </div>
    </div>

    <!-- Content grid -->
    <div class="content-grid">
      <!-- EPG Timeline -->
      <div class="epg-section">
        <button class="section-header" type="button" onclick={() => (epgCollapsed = !epgCollapsed)}>
          <h2>Program Guide</h2>
          <span class="material-icons-outlined toggle-icon" class:collapsed={epgCollapsed}>
            expand_more
          </span>
        </button>
        {#if !epgCollapsed}
          <EpgTimeline {programs} />
        {/if}
      </div>

      <!-- Sources -->
      <div class="sources-section">
        <h2>Sources ({sources.length})</h2>
        {#if sources.length === 0}
          <p class="muted-text">No sources available</p>
        {:else}
          <div class="source-list">
            {#each sources as source (source.id)}
              <button
                class="source-item"
                onclick={() => playback.play(channelId, source.id)}
                title="Play this source"
              >
                <div class="source-info">
                  <span class="source-name">{source.name ?? 'Source'}</span>
                  <div class="source-badges">
                    <span class="badge quality">{source.quality}</span>
                    <span class="badge type">{source.stream_type}</span>
                    <span class="badge priority">P{source.priority}</span>
                  </div>
                </div>
                <div class="source-health">
                  <div class="health-bar">
                    <div
                      class="health-fill"
                      style="width: {source.health_score}%; background: {healthColor(
                        source.health_score
                      )};"
                    ></div>
                  </div>
                  <span class="health-label">{source.health_score}%</span>
                </div>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .back-bar {
    margin-bottom: 16px;
  }

  .back-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .back-btn:hover {
    color: var(--ink);
    border-color: var(--accent);
  }

  .back-btn :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .player-section {
    margin-bottom: 16px;
  }

  .player-loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    aspect-ratio: 16 / 9;
    background: #000;
    border-radius: 12px;
  }

  .info-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    margin-bottom: 24px;
    flex-wrap: wrap;
  }

  .info-left {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .channel-icon {
    width: 48px;
    height: 48px;
    border-radius: 8px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .channel-icon-placeholder {
    width: 48px;
    height: 48px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--muted);
  }

  :global([data-theme='light']) .channel-icon-placeholder {
    background: rgba(0, 0, 0, 0.05);
  }

  .info-text {
    min-width: 0;
  }

  .info-text h1 {
    font-size: 18px;
    font-weight: 700;
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now-playing {
    font-size: 13px;
    color: var(--accent);
    display: block;
    margin-top: 2px;
  }

  .channel-meta {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }

  .meta-tag {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
  }

  :global([data-theme='light']) .meta-tag {
    background: rgba(0, 0, 0, 0.06);
  }

  .info-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  .action-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .action-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .action-btn.active {
    color: #ef4444;
    border-color: rgba(239, 68, 68, 0.3);
    background: rgba(239, 68, 68, 0.08);
  }

  .action-btn :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .content-grid {
    display: grid;
    grid-template-columns: 1fr 320px;
    gap: 24px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    margin-bottom: 12px;
    color: inherit;
    font: inherit;
  }

  .section-header h2 {
    font-size: 15px;
    font-weight: 600;
    margin: 0;
  }

  .toggle-icon {
    font-size: 20px;
    color: var(--muted);
    transition: transform 0.15s;
  }

  .toggle-icon.collapsed {
    transform: rotate(-90deg);
  }

  .sources-section h2 {
    font-size: 15px;
    font-weight: 600;
    margin: 0 0 12px;
  }

  .muted-text {
    color: var(--muted);
    font-size: 13px;
    margin: 0;
  }

  .source-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .source-item {
    padding: 10px 12px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--ring);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 100%;
    text-align: left;
    font: inherit;
    color: inherit;
    cursor: pointer;
    transition: background 0.15s;
  }

  .source-item:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  :global([data-theme='light']) .source-item {
    background: rgba(0, 0, 0, 0.02);
  }

  :global([data-theme='light']) .source-item:hover {
    background: rgba(0, 0, 0, 0.06);
  }

  .source-info {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .source-name {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .source-badges {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 4px;
    font-weight: 600;
    text-transform: uppercase;
  }

  .badge.quality {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .badge.type {
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
  }

  :global([data-theme='light']) .badge.type {
    background: rgba(0, 0, 0, 0.06);
  }

  .badge.priority {
    background: rgba(255, 255, 255, 0.06);
    color: var(--muted);
  }

  :global([data-theme='light']) .badge.priority {
    background: rgba(0, 0, 0, 0.04);
  }

  .source-health {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .health-bar {
    flex: 1;
    height: 4px;
    background: var(--ring);
    border-radius: 2px;
    overflow: hidden;
  }

  .health-fill {
    height: 100%;
    border-radius: 2px;
    transition: width 0.3s;
  }

  .health-label {
    font-size: 11px;
    color: var(--muted);
    width: 32px;
    text-align: right;
    flex-shrink: 0;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 80px;
  }

  .loading-text {
    font-size: 13px;
    color: var(--muted);
  }

  .spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (max-width: 768px) {
    .content-grid {
      grid-template-columns: 1fr;
    }

    .info-bar {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
