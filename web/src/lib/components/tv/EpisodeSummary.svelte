<script lang="ts">
  import { formatCalendarDate } from '$lib/utils/format';

  interface Props {
    number: string | number;
    title: string;
    airDate?: string | null;
    runtime?: number | null;
    stillUrl?: string | null;
    synopsis?: string | null;
  }

  let { number, title, airDate, runtime, stillUrl, synopsis }: Props = $props();

  const formattedAirDate = $derived(formatCalendarDate(airDate ?? null, 'short'));
</script>

<div class="episode-number">{number}</div>
{#if stillUrl}
  <img class="episode-still" src={stillUrl} alt="" loading="lazy" />
{/if}
<div class="episode-info">
  <span class="episode-title">{title}</span>
  <div class="episode-meta">
    {#if formattedAirDate}<span>{formattedAirDate}</span>{/if}
    {#if runtime}<span>{runtime}m</span>{/if}
  </div>
  {#if synopsis}<p class="episode-synopsis">{synopsis}</p>{/if}
</div>

<style>
  .episode-number {
    min-width: 32px;
    font-size: 13px;
    font-weight: 700;
    color: var(--muted);
    text-align: center;
    flex-shrink: 0;
  }

  .episode-still {
    width: 128px;
    aspect-ratio: 16 / 9;
    object-fit: cover;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    flex-shrink: 0;
  }

  .episode-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .episode-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    transition: color 0.15s ease;
  }

  .episode-meta {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: var(--muted);
  }

  .episode-synopsis {
    margin: 2px 0 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--muted);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  @media (max-width: 480px) {
    .episode-number {
      min-width: 24px;
      font-size: 12px;
    }

    .episode-still {
      width: 88px;
    }

    .episode-synopsis {
      display: none;
    }
  }
</style>
