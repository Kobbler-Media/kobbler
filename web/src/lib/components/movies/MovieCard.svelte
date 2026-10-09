<script lang="ts">
  import type { Movie } from '$lib/types';
  import { movieStatusChip } from '$lib/utils/chip-mappings';
  import IconChip from '$lib/components/shared/IconChip.svelte';
  import MediaCard from '$lib/components/shared/MediaCard.svelte';
  import { isActivelyWanted } from '$lib/utils/want-status';

  interface Props {
    movie: Movie;
    selected: boolean;
    onSelect: (movie: Movie) => void;
  }

  let { movie, selected, onSelect }: Props = $props();

  const isWanted = $derived(isActivelyWanted(movie.want_status));

  const statusBadge = $derived.by(() => {
    if (isWanted) return { label: 'Wanted', class: 'wanted' };
    if (movie.is_in_library) return null;
    return { label: 'Missing', class: 'pending' };
  });

  const posterUrl = $derived(
    movie.poster_url ? `${movie.poster_url}?t=${encodeURIComponent(movie.updated_at || '')}` : ''
  );
</script>

<MediaCard
  {posterUrl}
  placeholderIcon="movie"
  {selected}
  wanted={isWanted}
  itemId={movie.id}
  onclick={() => onSelect(movie)}
>
  {#snippet posterOverlay()}
    {#if movie.watched === true}
      <span class="chip-pos-left">
        <IconChip icon="visibility" label="Watched" color="var(--success, #22c55e)" />
      </span>
    {/if}
    {#if statusBadge}
      {@const chip = movieStatusChip(statusBadge.class)}
      <span class="chip-pos">
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
      </span>
    {/if}
  {/snippet}
  <strong class="card-title">{movie.title}</strong>
  {#if movie.year}
    <div class="card-sub">{movie.year}</div>
  {/if}
  {#if movie.genres?.length > 0}
    <div class="card-accent">{movie.genres.slice(0, 2).join(', ')}</div>
  {/if}
</MediaCard>
