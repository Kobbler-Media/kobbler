<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string;
    backdropUrl?: string | null;
    posterUrl?: string | null;
    /** Page-level backdrop image shown behind all children content (used with noHeader). */
    pageBackdropUrl?: string | null;
    onBack?: () => void;
    onClose: () => void;
    onEdit?: () => void;
    onRefresh?: () => void;
    /** When true, the refresh icon spins to indicate enrichment is in progress. */
    syncing?: boolean;
    headerActions?: Snippet;
    bannerContent?: Snippet;
    children: Snippet;
    footer?: Snippet;
    /** If true, skip the default header/banner (use when an EntityHero is rendered inside). */
    noHeader?: boolean;
    /** If true, skip internal padding in the inner content area (useful for full-width footers). */
    noPadding?: boolean;
  }

  let {
    title = 'Details',
    backdropUrl,
    posterUrl,
    pageBackdropUrl,
    onBack,
    onClose,
    onEdit,
    onRefresh,
    syncing = false,
    headerActions,
    bannerContent,
    children,
    footer,
    noHeader = false,
    noPadding = false
  }: Props = $props();
</script>

<div class="content-page">
  <div class="content-page-body">
    {#if noHeader}
      {#if pageBackdropUrl}
        <div class="page-backdrop" style="background-image: url({pageBackdropUrl})"></div>
      {/if}
      <!-- Floating actions for when we use EntityHero which handles its own top area -->
      <div class="floating-actions floating-left">
        {#if onBack}
          <button type="button" class="glass-btn" onclick={onBack} title="Back">
            <span class="material-icons-outlined">arrow_back</span>
          </button>
        {/if}
      </div>
      <div class="floating-actions floating-right">
        {#if headerActions}
          {@render headerActions()}
        {:else}
          {#if onRefresh}
            <button
              type="button"
              class="glass-btn"
              class:syncing
              onclick={onRefresh}
              title={syncing ? 'Syncing metadata...' : 'Refresh'}
            >
              <span class="material-icons-outlined" class:spin={syncing}>refresh</span>
            </button>
          {/if}
          {#if onEdit}
            <button type="button" class="glass-btn" onclick={onEdit} title="Edit">
              <span class="material-icons-outlined">edit</span>
            </button>
          {/if}
          <button type="button" class="glass-btn" onclick={onClose} title="Close">
            <span class="material-icons-outlined">close</span>
          </button>
        {/if}
      </div>

      <div
        class={noPadding ? 'content-page-no-header' : 'content-page-no-header content-page-inner'}
      >
        {@render children()}
      </div>
    {:else}
      {#if backdropUrl}
        <div class="banner" style="background-image: url({backdropUrl})">
          <div class="banner-gradient"></div>
          <div class="floating-actions floating-left">
            {#if onBack}
              <button type="button" class="glass-btn" onclick={onBack} title="Back">
                <span class="material-icons-outlined">arrow_back</span>
              </button>
            {/if}
          </div>
          <div class="floating-actions floating-right">
            {#if headerActions}
              {@render headerActions()}
            {:else}
              {#if onRefresh}
                <button type="button" class="glass-btn" onclick={onRefresh} title="Refresh">
                  <span class="material-icons-outlined">refresh</span>
                </button>
              {/if}
              {#if onEdit}
                <button type="button" class="glass-btn" onclick={onEdit} title="Edit">
                  <span class="material-icons-outlined">edit</span>
                </button>
              {/if}
              <button type="button" class="glass-btn" onclick={onClose} title="Close">
                <span class="material-icons-outlined">close</span>
              </button>
            {/if}
          </div>
          <div class="banner-content">
            {#if posterUrl}
              <img class="banner-poster" src={posterUrl} alt="" />
            {/if}
            {#if bannerContent}
              <div class="banner-info">
                {@render bannerContent()}
              </div>
            {/if}
          </div>
        </div>
      {:else}
        <header class="content-page-header">
          <div class="header-left">
            {#if onBack}
              <button type="button" class="icon-btn" onclick={onBack} title="Back">
                <span class="material-icons-outlined">arrow_back</span>
              </button>
            {/if}
            <h1 class="header-title">{title}</h1>
          </div>
          <div class="header-actions">
            {#if headerActions}
              {@render headerActions()}
            {:else}
              {#if onRefresh}
                <button
                  type="button"
                  class="icon-btn"
                  class:syncing
                  onclick={onRefresh}
                  title={syncing ? 'Syncing metadata...' : 'Refresh'}
                >
                  <span class="material-icons-outlined" class:spin={syncing}>refresh</span>
                </button>
              {/if}
              {#if onEdit}
                <button type="button" class="icon-btn" onclick={onEdit} title="Edit">
                  <span class="material-icons-outlined">edit</span>
                </button>
              {/if}
              <button type="button" class="icon-btn" onclick={onClose} title="Close">
                <span class="material-icons-outlined">close</span>
              </button>
            {/if}
          </div>
        </header>
      {/if}

      <div class={noPadding ? '' : 'content-page-inner'}>
        {@render children()}
      </div>
    {/if}
  </div>

  {#if footer}
    <footer class="content-page-footer">
      {@render footer()}
    </footer>
  {/if}
</div>

<style>
  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .syncing {
    opacity: 0.7;
    pointer-events: none;
  }

  .content-page {
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .content-page-body {
    flex: 1;
    position: relative;
  }

  .content-page-footer {
    flex-shrink: 0;
    padding: 16px 40px;
    background: linear-gradient(180deg, rgba(17, 20, 25, 0.85), rgba(21, 24, 30, 0.95));
    backdrop-filter: blur(20px) saturate(1.8);
    -webkit-backdrop-filter: blur(20px) saturate(1.8);
    border-top: 1px solid var(--ring);
    z-index: 20;
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.3);
    position: sticky;
    bottom: 0;
  }

  :global([data-theme='light']) .content-page-footer {
    background: linear-gradient(180deg, rgba(239, 230, 219, 0.85), rgba(245, 239, 231, 0.95));
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.05);
  }

  /* --- No-backdrop header --- */
  .content-page-header {
    padding: 0 20px;
    height: 56px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
    position: sticky;
    top: 0;
    background: var(--bg);
    z-index: 20;
    border-bottom: 1px solid var(--ring);
    box-sizing: border-box;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .header-title {
    font-size: 24px;
    font-weight: 900;
    margin: 0;
    color: var(--ink);
    letter-spacing: -0.02em;
    line-height: 32px;
  }

  .header-actions {
    display: flex;
    gap: 8px;
  }

  .icon-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.2s;
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
    transform: translateY(-1px);
  }

  .content-page-inner {
    padding: 40px;
    max-width: 1600px;
    margin: 0 auto;
  }

  /* --- Full-bleed banner with backdrop --- */
  .banner {
    position: relative;
    width: 100%;
    min-height: 400px;
    background-size: cover;
    background-position: center top;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
  }

  .banner-gradient {
    position: absolute;
    inset: 0;
    background: linear-gradient(
      to bottom,
      rgba(0, 0, 0, 0.2) 0%,
      rgba(0, 0, 0, 0.6) 60%,
      var(--bg) 100%
    );
  }

  /* Page-level backdrop — single masked element behind all children */
  .page-backdrop {
    position: absolute;
    top: 0;
    left: 0;
    right: -20px;
    height: 600px;
    background-size: cover;
    background-position: center 20%;
    filter: blur(60px) saturate(1.8);
    opacity: 0.35;
    pointer-events: none;
    z-index: 0;
    -webkit-mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
    mask-image: linear-gradient(to bottom, black 0%, transparent 100%);
  }

  :global([data-theme='light']) .page-backdrop {
    opacity: 0.15;
    filter: blur(80px) saturate(1.2);
  }

  .content-page-no-header {
    position: relative;
    z-index: 2;
  }

  /* Floating glass action buttons */
  .floating-actions {
    position: absolute;
    top: 12px;
    z-index: 20;
    display: flex;
    gap: 8px;
  }

  .floating-left {
    left: 20px;
  }

  .floating-right {
    right: 20px;
  }

  .banner-content {
    position: relative;
    z-index: 1;
    display: flex;
    gap: 32px;
    align-items: flex-end;
    padding: 40px;
    max-width: 1400px;
    margin: 0 auto;
    width: 100%;
  }

  .banner-poster {
    width: 200px;
    border-radius: 16px;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.1);
    flex-shrink: 0;
  }

  .banner-info {
    flex: 1;
    min-width: 0;
  }

  @media (max-width: 768px) {
    .content-page-header {
      padding: 12px 16px;
    }

    .content-page-inner {
      padding: 16px;
    }

    .banner-content {
      flex-direction: column;
      align-items: center;
      text-align: center;
      padding: 24px 16px;
      padding-top: 80px;
    }

    .banner-poster {
      width: 120px;
    }

    .floating-left {
      left: 10px;
    }

    .floating-right {
      right: 10px;
    }

    .content-page-footer {
      padding: 12px 16px;
    }
  }
</style>
