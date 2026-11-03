<script lang="ts">
  import type { DiscoverEpisode } from '#lib/api/client.js';
  import SeasonAccordionShell from '../tv/SeasonAccordionShell.svelte';
  import DiscoverEpisodeRow from './DiscoverEpisodeRow.svelte';
  import { listedSeasons, type DiscoverSeason, type EpisodeRef } from './discoverDetail';
  import type { useDiscoverSeasons } from './useDiscoverSeasons.svelte';

  interface Props {
    seasons: DiscoverSeason[];
    loader: ReturnType<typeof useDiscoverSeasons>;
    initiallyExpanded: number | null;
    canStream: boolean;
    isWanting: (ref: EpisodeRef) => boolean;
    onStream: (ref: EpisodeRef) => void;
    onChooseRelease: (ref: EpisodeRef) => void;
    onWant: (ref: EpisodeRef) => void;
  }

  let {
    seasons,
    loader,
    initiallyExpanded,
    canStream,
    isWanting,
    onStream,
    onChooseRelease,
    onWant
  }: Props = $props();

  let expanded = $state(new Set<number>());
  let seeded = false;

  $effect(() => {
    if (seeded || initiallyExpanded === null) return;
    seeded = true;
    expanded = new Set([initiallyExpanded]);
    void loader.load(initiallyExpanded);
  });

  const ordered = $derived(listedSeasons(seasons));

  function toggle(season: number) {
    const next = new Set(expanded);
    if (next.has(season)) {
      next.delete(season);
    } else {
      next.add(season);
      void loader.load(season);
    }
    expanded = next;
  }

  const label = (s: DiscoverSeason) =>
    s.title ?? (s.season_number === 0 ? 'Specials' : `Season ${s.season_number}`);
</script>

{#snippet rows(season: number, episodes: DiscoverEpisode[])}
  {#each episodes as ep (ep.episode_number)}
    {@const ref = { season, episode: ep.episode_number }}
    <DiscoverEpisodeRow
      episode={ep}
      {canStream}
      wanting={isWanting(ref)}
      onStream={() => onStream(ref)}
      onChooseRelease={() => onChooseRelease(ref)}
      onWant={() => onWant(ref)}
    />
  {/each}
{/snippet}

{#if ordered.length === 0}
  <p class="empty">No seasons listed.</p>
{:else}
  <div class="seasons-list">
    {#each ordered as season (season.season_number)}
      {@const n = season.season_number}
      <SeasonAccordionShell
        label={label(season)}
        expanded={expanded.has(n)}
        onToggle={() => toggle(n)}
      >
        {#snippet meta()}
          {#if season.episode_count}<span>{season.episode_count} episodes</span>{/if}
        {/snippet}
        {#if loader.episodes.has(n)}
          {@const episodes = loader.episodes.get(n) ?? []}
          {#if episodes.length === 0}
            <div class="empty">No episodes found</div>
          {:else}
            {@render rows(n, episodes)}
          {/if}
        {:else if loader.hasFailed(n)}
          <div class="empty">Couldn't load this season</div>
        {:else}
          <div class="empty"><span class="spinner"></span></div>
        {/if}
      </SeasonAccordionShell>
    {/each}
  </div>
{/if}

<style>
  .seasons-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .empty {
    padding: 20px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }

  .spinner {
    display: inline-block;
    width: 18px;
    height: 18px;
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
</style>
