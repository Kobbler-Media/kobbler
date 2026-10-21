<script lang="ts">
  import DetailSection from './DetailSection.svelte';
  import type { ReleaseDates } from '$lib/types';
  import {
    browserRegion,
    releaseAvailability,
    releaseRegionNote,
    todayIso,
    type LibraryStatus,
    type StreamingAvailability
  } from '$lib/utils/releaseAvailability';

  interface Props {
    releaseDate: string | null;
    releaseDates: ReleaseDates | null;
    year: number | null;
    streaming: StreamingAvailability;
    library: LibraryStatus;
  }

  let { releaseDate, releaseDates, year, streaming, library }: Props = $props();

  const rows = $derived(
    releaseAvailability({ releaseDate, releaseDates, year, today: todayIso(), streaming, library })
  );
  const regionNote = $derived(
    releaseRegionNote(
      releaseDates,
      typeof navigator === 'undefined' ? null : browserRegion(navigator.languages)
    )
  );
</script>

<DetailSection label="Release availability">
  <dl class="rows">
    {#each rows as row (row.label)}
      <div class="row">
        <dt>{row.label}</dt>
        <dd class:emphasised={row.emphasised}>{row.value}</dd>
      </div>
    {/each}
  </dl>
  {#if regionNote}
    <p class="region-note">{regionNote}</p>
  {/if}
</DetailSection>

<style>
  .rows {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
  }

  .row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 14px;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
    color: var(--ink);
  }

  .region-note {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--muted);
  }

  dd.emphasised {
    color: var(--accent);
    font-weight: 600;
  }
</style>
