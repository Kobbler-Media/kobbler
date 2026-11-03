<script lang="ts">
  import type { CachingState } from '#lib/streaming/cacheWait.js';

  interface Props {
    title: string;
    subtitle?: string;
    artUrl?: string;
    status?: string | null;
    caching?: CachingState | null;
    onCancel: () => void;
    onChooseRelease?: () => void;
  }

  let {
    title,
    subtitle,
    artUrl,
    status,
    caching = null,
    onCancel,
    onChooseRelease
  }: Props = $props();

  const percent = $derived(caching ? Math.round(caching.progress * 100) : 0);
  const cachingLine = $derived(
    caching ? [percent > 0 ? `${percent}%` : null, caching.note].filter(Boolean).join(' · ') : ''
  );
</script>

<div class="start-shell" class:has-art={!!artUrl}>
  {#if artUrl}
    <div class="art" style="background-image: url({artUrl})"></div>
  {/if}
  <div class="content">
    {#if caching}
      <h2 class="heading">Preparing stream</h2>
      {#if title}<p class="sub">{title}</p>{/if}
      <div class="bar" class:indeterminate={percent === 0}>
        <div class="fill" style="width: {percent === 0 ? 30 : percent}%"></div>
      </div>
      <p class="note">{cachingLine}</p>
    {:else}
      <span class="material-icons-outlined spinner" aria-hidden="true">refresh</span>
      <h2 class="heading">{title || 'Connecting…'}</h2>
      {#if subtitle}<p class="sub">{subtitle}</p>{/if}
      {#if status}<p class="note" aria-live="polite">{status}</p>{/if}
    {/if}
    <div class="actions">
      <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
      {#if caching && onChooseRelease}
        <button type="button" class="ghost" onclick={onChooseRelease}>
          Choose another release
        </button>
      {/if}
    </div>
  </div>
</div>

<style>
  .start-shell {
    position: absolute;
    inset: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.85);
    animation: fade-in 0.2s ease;
  }

  .start-shell.has-art {
    background: #000;
  }

  .art {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center;
    opacity: 0.4;
  }

  .content {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    width: min(420px, calc(100% - 32px));
    text-align: center;
    color: #fff;
  }

  .spinner {
    font-size: 40px;
    color: rgba(255, 255, 255, 0.85);
    animation: spin 1s linear infinite;
    margin-bottom: 8px;
  }

  .heading {
    margin: 0;
    font-size: 20px;
    font-weight: 700;
    text-shadow: 0 1px 8px rgba(0, 0, 0, 0.6);
  }

  .sub,
  .note {
    margin: 0;
    font-size: 13px;
    color: rgba(255, 255, 255, 0.72);
  }

  .note {
    min-height: 1.4em;
    font-variant-numeric: tabular-nums;
  }

  .bar {
    position: relative;
    width: 100%;
    height: 3px;
    margin-top: 12px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.15);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.4s ease;
  }

  .bar.indeterminate .fill {
    position: absolute;
    animation: slide 1.4s ease-in-out infinite;
  }

  .actions {
    display: flex;
    gap: 8px;
    margin-top: 16px;
  }

  .ghost {
    padding: 8px 18px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.85);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }

  .ghost:hover {
    background: rgba(255, 255, 255, 0.18);
    color: #fff;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes slide {
    from {
      left: -30%;
    }
    to {
      left: 100%;
    }
  }

  @keyframes fade-in {
    from {
      opacity: 0;
    }
  }
</style>
