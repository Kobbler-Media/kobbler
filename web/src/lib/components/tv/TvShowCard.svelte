<script lang="ts">
  import type { TvShow } from '#lib/types.js';
  import { tvStatusChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import MediaCard from '#lib/components/shared/MediaCard.svelte';

  interface Props {
    show: TvShow;
    selected?: boolean;
    selectionMode?: boolean;
    onSelect: (show: TvShow) => void;
    onToggleSelect?: (show: TvShow) => void;
  }

  let { show, selected = false, selectionMode = false, onSelect, onToggleSelect }: Props = $props();

  const statusBadge = $derived.by(() => {
    if (show.is_monitored && show.status === 'ended') {
      return { label: 'Ended', class: 'ended' };
    }
    if (show.is_monitored) {
      return { label: 'Monitored', class: 'monitored' };
    }
    return null;
  });

  const yearDisplay = $derived(show.year ? `(${show.year})` : '');

  const aired = $derived(show.aired_episode_count ?? 0);
  const inLibrary = $derived(show.in_library_count ?? 0);
  const progressPercent = $derived(aired > 0 ? (inLibrary / aired) * 100 : 0);
  const progressTitle = $derived(
    aired > 0 ? `${inLibrary} / ${aired} aired episodes in library` : null
  );

  const posterUrl = $derived(
    show.poster_url ? `${show.poster_url}?t=${encodeURIComponent(show.updated_at || '')}` : null
  );

  function handleClick() {
    if (selectionMode) {
      onToggleSelect?.(show);
    } else {
      onSelect(show);
    }
  }
</script>

<MediaCard {posterUrl} placeholderIcon="tv" {selected} itemId={show.id} onclick={handleClick}>
  {#snippet posterOverlay()}
    {#if statusBadge}
      {@const chip = tvStatusChip(statusBadge.class)}
      <span class="chip-pos">
        <IconChip icon={chip.icon} label={chip.label} color={chip.color} />
      </span>
    {/if}
    {#if selectionMode}
      <span class="select-check" class:checked={selected}>
        {#if selected}
          <span class="material-icons-outlined" style="font-size: 18px;">check</span>
        {/if}
      </span>
    {/if}
    {#if aired > 0}
      <div class="progress-track" title={progressTitle}>
        <div
          class="progress-fill"
          class:complete={progressPercent >= 100}
          style="width: {Math.min(progressPercent, 100)}%"
        ></div>
      </div>
    {/if}
  {/snippet}
  <strong class="card-title">{show.title}</strong>
  {#if yearDisplay}
    <div class="card-sub">{yearDisplay}</div>
  {/if}
</MediaCard>

<style>
  .select-check {
    position: absolute;
    top: 8px;
    left: 8px;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    border: 2px solid rgba(255, 255, 255, 0.6);
    background: rgba(0, 0, 0, 0.3);
    display: flex;
    align-items: center;
    justify-content: center;
    color: white;
  }

  .select-check.checked {
    background: var(--accent);
    border-color: var(--accent);
  }
</style>
