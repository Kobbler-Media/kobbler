<script lang="ts">
  import MetaLine from '$lib/components/shared/MetaLine.svelte';

  interface Props {
    title: string;
    times: string;
    sub: (string | null | undefined)[];
    nextLine: string;
    leftLine: string;
    bufferedTitle: string;
  }

  let { title, times, sub, nextLine, leftLine, bufferedTitle }: Props = $props();
</script>

<div class="programme">
  <div class="body">
    <div class="line">
      <span class="buffered" title={bufferedTitle}>
        <span class="material-icons-outlined">history</span>
        BUFFERED
      </span>
      <span class="title">{title}</span>
      {#if times}<span class="times">{times}</span>{/if}
      {#if sub.some(Boolean)}<span class="sub"><MetaLine items={sub} /></span>{/if}
    </div>
    {#if nextLine}
      <span class="next">{nextLine}</span>
    {/if}
  </div>
  {#if leftLine}
    <span class="left">{leftLine}</span>
  {/if}
</div>

<style>
  .programme {
    display: flex;
    align-items: flex-end;
    gap: 16px;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
    flex-grow: 1;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .buffered {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    color: var(--accent);
    flex-shrink: 0;
    cursor: default;
  }

  .buffered .material-icons-outlined {
    font-size: 13px;
  }

  .title {
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .times {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .sub {
    font-size: 12px;
    color: var(--muted);
  }

  .next {
    font-size: 12.5px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .left {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  @media (max-width: 720px) {
    .title {
      font-size: 16px;
    }

    .left {
      display: none;
    }
  }
</style>
