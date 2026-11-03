<script lang="ts">
  import type { CalendarItem } from '#lib/types.js';
  import { mediaTypeChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';

  interface Props {
    item: CalendarItem;
  }

  let { item }: Props = $props();

  const chip = $derived(mediaTypeChip(item.media_type));

  const imageUrl = $derived(item.image_url ? `${item.image_url}` : null);
</script>

<a href={item.link} class="calendar-card" data-item-id={item.id} data-media-type={item.media_type}>
  <div class="poster" style={imageUrl ? `background-image: url(${imageUrl});` : ''}>
    {#if !imageUrl}
      <span class="material-icons-outlined placeholder-icon">{chip.icon}</span>
    {/if}
  </div>
  <div class="info">
    <div class="title-row">
      <span class="title">{item.title}</span>
      <IconChip icon={chip.icon} label={chip.label} color={chip.color} size="sm" />
    </div>
    {#if item.subtitle}
      <span class="subtitle">{item.subtitle}</span>
    {/if}
  </div>
</a>

<style>
  .calendar-card {
    display: flex;
    gap: 14px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    text-decoration: none;
    color: inherit;
    transition:
      transform 0.12s ease,
      box-shadow 0.12s ease,
      border-color 0.12s ease;
    min-width: 0;
  }

  .calendar-card:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
    border-color: var(--accent);
  }

  :global([data-theme='light']) .calendar-card:hover {
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.1);
  }

  .poster {
    flex-shrink: 0;
    width: 56px;
    height: 84px;
    border-radius: 8px;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    background-size: cover;
    background-position: center;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border: 1px solid var(--ring);
  }

  :global([data-theme='light']) .poster {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
    background-size: cover;
    background-position: center;
  }

  .placeholder-icon {
    font-size: 26px;
    color: var(--muted);
    opacity: 0.4;
  }

  .info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    justify-content: center;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .title {
    font-size: 15px;
    font-weight: 700;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .subtitle {
    font-size: 13px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  @media (max-width: 768px) {
    .calendar-card {
      gap: 10px;
      padding: 8px;
    }

    .poster {
      width: 44px;
      height: 66px;
    }

    .title {
      font-size: 14px;
    }

    .subtitle {
      font-size: 12px;
    }
  }
</style>
