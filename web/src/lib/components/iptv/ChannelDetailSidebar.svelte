<script lang="ts">
  import type { IptvChannel, IptvSource, IptvEpgEntry } from '#lib/types.js';
  import {
    getIptvChannelSources,
    getIptvChannelEpg,
    getIptvFavourites,
    addIptvFavourite,
    removeIptvFavourite
  } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import EpgTimeline from './EpgTimeline.svelte';
  import RecordButton from './RecordButton.svelte';

  interface Props {
    channel: IptvChannel;
    onPlay: () => void;
    onPlaySource?: (sourceId: string) => void;
  }

  let { channel, onPlay, onPlaySource }: Props = $props();

  let sources = $state<IptvSource[]>([]);
  let programs = $state<IptvEpgEntry[]>([]);
  let isFavourite = $state(false);
  let loadingSources = $state(true);
  let epgCollapsed = $state(false);

  async function loadDetails() {
    loadingSources = true;
    try {
      const [sourcesRes, epgRes, favs] = await Promise.all([
        getIptvChannelSources(channel.id),
        getIptvChannelEpg(channel.id),
        getIptvFavourites()
      ]);
      sources = sourcesRes;
      programs = epgRes;
      isFavourite = favs.some((f) => f.id === channel.id);
    } catch (e) {
      console.error('Failed to load channel details:', e);
    } finally {
      loadingSources = false;
    }
  }

  async function toggleFavourite() {
    try {
      if (isFavourite) {
        await removeIptvFavourite(channel.id);
        isFavourite = false;
        showMessage('Removed from favourites');
      } else {
        await addIptvFavourite(channel.id);
        isFavourite = true;
        showMessage('Added to favourites');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update favourite');
    }
  }

  let currentProgram = $derived(
    programs.find((p) => {
      const now = Date.now();
      return new Date(p.start_time).getTime() <= now && new Date(p.stop_time).getTime() > now;
    })
  );

  function healthColor(score: number): string {
    if (score >= 80) return 'var(--success, #22c55e)';
    if (score >= 50) return 'var(--warning, #f59e0b)';
    return 'var(--danger, #ef4444)';
  }

  $effect(() => {
    channel.id;
    loadDetails();
  });
</script>

<div class="channel-sidebar">
  <!-- Channel header -->
  <div class="channel-header">
    {#if channel.icon_url}
      <img src={channel.icon_url} alt="" class="channel-icon" />
    {:else}
      <div class="channel-icon-placeholder">
        <span class="material-icons-outlined">live_tv</span>
      </div>
    {/if}
    <div class="header-info">
      <h3 class="channel-name">{channel.name}</h3>
      {#if channel.current_program}
        <span class="now-playing">{channel.current_program}</span>
      {/if}
      <div class="channel-tags">
        {#if channel.category}
          <span class="tag">{channel.category}</span>
        {/if}
        {#if channel.language}
          <span class="tag">{channel.language}</span>
        {/if}
      </div>
    </div>
  </div>

  <!-- Action buttons -->
  <div class="actions">
    <button class="btn-primary play-btn" type="button" onclick={onPlay}>
      <span class="material-icons-outlined">play_arrow</span>
      Watch Live
    </button>
    <button
      class="icon-action"
      class:active={isFavourite}
      type="button"
      aria-label={isFavourite ? 'Remove from favourites' : 'Add to favourites'}
      onclick={toggleFavourite}
    >
      <span class="material-icons-outlined">{isFavourite ? 'favorite' : 'favorite_border'}</span>
    </button>
    <RecordButton
      channelId={channel.id}
      title={currentProgram?.title ?? channel.current_program ?? channel.name}
      epgId={currentProgram?.id}
    />
  </div>

  <!-- EPG section -->
  <section class="detail-section">
    <button class="section-header" type="button" onclick={() => (epgCollapsed = !epgCollapsed)}>
      <h4>Program Guide</h4>
      <span class="material-icons-outlined toggle-icon" class:collapsed={epgCollapsed}>
        expand_more
      </span>
    </button>
    {#if !epgCollapsed}
      <EpgTimeline {programs} />
    {/if}
  </section>

  <!-- Sources section -->
  <section class="detail-section">
    <h4>Sources ({sources.length})</h4>
    {#if loadingSources}
      <p class="muted-text">Loading...</p>
    {:else if sources.length === 0}
      <p class="muted-text">No sources available</p>
    {:else}
      <div class="source-list">
        {#each sources as source (source.id)}
          <button
            class="source-item"
            onclick={() => onPlaySource?.(source.id)}
            title="Play this source"
          >
            <div class="source-info">
              <span class="source-name">{source.name ?? 'Source'}</span>
              <div class="source-badges">
                {#if source.provider_name}
                  <span class="badge provider">{source.provider_name}</span>
                {/if}
                <span class="badge quality">{source.quality}</span>
                <span class="badge type">{source.stream_type}</span>
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
  </section>
</div>

<style>
  .channel-sidebar {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .channel-header {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .channel-icon {
    width: 56px;
    height: 56px;
    border-radius: 10px;
    object-fit: contain;
    flex-shrink: 0;
    background: rgba(255, 255, 255, 0.05);
  }

  :global([data-theme='light']) .channel-icon {
    background: rgba(0, 0, 0, 0.05);
  }

  .channel-icon-placeholder {
    width: 56px;
    height: 56px;
    border-radius: 10px;
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

  .header-info {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .channel-name {
    font-size: 16px;
    font-weight: 700;
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .now-playing {
    font-size: 13px;
    color: var(--accent);
  }

  .channel-tags {
    display: flex;
    gap: 6px;
    margin-top: 2px;
  }

  .tag {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
  }

  :global([data-theme='light']) .tag {
    background: rgba(0, 0, 0, 0.06);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .play-btn {
    flex: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }

  .icon-action {
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    flex-shrink: 0;
  }

  .icon-action:hover {
    color: var(--ink);
    border-color: var(--accent);
  }

  .icon-action.active {
    color: #ef4444;
    border-color: rgba(239, 68, 68, 0.3);
  }

  .icon-action :global(.material-icons-outlined) {
    font-size: 20px;
  }

  .detail-section {
    border-top: 1px solid var(--ring);
    padding-top: 12px;
  }

  .detail-section h4 {
    font-size: 13px;
    font-weight: 600;
    margin: 0 0 8px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
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
    margin-bottom: 8px;
    color: inherit;
  }

  .section-header h4 {
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

  .muted-text {
    font-size: 13px;
    color: var(--muted);
    margin: 0;
  }

  .source-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .source-item {
    padding: 8px 10px;
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

  .badge.provider {
    background: rgba(139, 92, 246, 0.15);
    color: rgb(167, 139, 250);
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
</style>
