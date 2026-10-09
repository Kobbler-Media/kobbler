<script lang="ts">
  interface Props {
    quality: string | null;
    source: string | null;
  }

  let { quality, source }: Props = $props();

  const sourceIcon = $derived.by(() => {
    if (!source) return null;
    switch (source) {
      case 'media_root':
        return 'folder';
      case 'kobload':
        return 'cloud_download';
      default:
        return 'source';
    }
  });
</script>

<span class="quality-badge">
  {#if sourceIcon}
    <span class="material-icons-outlined source-icon" title={source}>{sourceIcon}</span>
  {/if}
  {#if quality}
    {quality}
  {/if}
</span>

<style>
  .quality-badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    color: var(--ink);
    white-space: nowrap;
  }

  .source-icon {
    font-size: 12px;
    color: var(--muted);
  }
</style>
