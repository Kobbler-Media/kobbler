<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    label: string;
    href?: string;
    expanded: boolean;
    onToggle: () => void;
    leading?: Snippet;
    meta?: Snippet;
    actions?: Snippet;
    children: Snippet;
  }

  let { label, href, expanded, onToggle, leading, meta, actions, children }: Props = $props();
</script>

<div class="season-accordion" class:expanded>
  <!-- A div rather than a button so the header can hold its own buttons. -->
  <div
    class="season-header"
    role="button"
    tabindex="0"
    aria-expanded={expanded}
    onclick={onToggle}
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        onToggle();
      }
    }}
  >
    {@render leading?.()}
    <span class="expand-icon material-icons-outlined">
      {expanded ? 'expand_more' : 'chevron_right'}
    </span>
    <div class="season-info">
      {#if href}
        <a class="season-title" {href} onclick={(e: MouseEvent) => e.stopPropagation()}>{label}</a>
      {:else}
        <span class="season-title">{label}</span>
      {/if}
      <div class="season-meta">{@render meta?.()}</div>
    </div>
    {@render actions?.()}
  </div>

  {#if expanded}
    <div class="season-episodes">{@render children()}</div>
  {/if}
</div>

<style>
  .season-accordion {
    container-type: inline-size;
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
    background: var(--card);
  }

  .season-accordion.expanded {
    border-color: var(--accent-dim);
  }

  .season-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 14px 16px;
    border: none;
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    font: inherit;
    text-align: left;
    transition: background 0.15s ease;
  }

  .season-header:hover {
    background: var(--bg-2);
  }

  .expand-icon {
    font-size: 18px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .season-info {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .season-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
  }

  .season-title {
    white-space: nowrap;
    font-size: 14px;
    font-weight: 700;
    color: var(--ink);
    text-decoration: none;
    transition: color 0.15s ease;
  }

  a.season-title:hover {
    color: var(--accent);
  }

  .season-episodes {
    border-top: 1px solid var(--ring);
  }

  @media (max-width: 768px) {
    .season-header {
      padding: 12px 10px;
    }
  }

  /* The seasons card sits in a narrow grid column at laptop widths, not just on phones. */
  @container (max-width: 560px) {
    .season-info {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }
  }
</style>
