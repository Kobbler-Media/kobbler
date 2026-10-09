<script lang="ts">
  import { page } from '$app/stores';
  import type { Snippet } from 'svelte';

  interface NavItem {
    href: string;
    label: string;
    icon: string;
    count?: number;
    badge?: number; // For special badge styling (e.g., ingest queue)
  }

  interface Props {
    title: string;
    items: NavItem[];
    expanded: boolean;
    collapsed: boolean;
    onToggle: () => void;
    headerExtra?: Snippet;
    children?: Snippet;
  }

  let { title, items, expanded, collapsed, onToggle, headerExtra, children }: Props = $props();

  function isActive(href: string): boolean {
    const url = new URL(href, 'http://x');
    if (!$page.url.pathname.startsWith(url.pathname)) return false;
    // If the nav item has query params, they must all match
    if (url.search) {
      for (const [key, value] of url.searchParams) {
        if ($page.url.searchParams.get(key) !== value) return false;
      }
      return true;
    }
    // Plain path item: not active if another item with query params on same path is active
    // (e.g., /library is not active when /library?has_audiobook=true is)
    const hasQuerySibling = items.some((item) => {
      const u = new URL(item.href, 'http://x');
      return u.pathname === url.pathname && u.search && item.href !== href;
    });
    if (hasQuerySibling) {
      // Only active if the current page has none of the sibling's distinguishing params
      for (const item of items) {
        const u = new URL(item.href, 'http://x');
        if (u.pathname === url.pathname && u.search && item.href !== href) {
          let allMatch = true;
          for (const [key, value] of u.searchParams) {
            if ($page.url.searchParams.get(key) !== value) {
              allMatch = false;
              break;
            }
          }
          if (allMatch) return false;
        }
      }
    }
    return true;
  }

  const hasActiveItem = $derived(items.some((item) => isActive(item.href)));
</script>

<div class="nav-group" class:collapsed>
  {#if !collapsed}
    <button class="group-header" type="button" onclick={onToggle} class:has-active={hasActiveItem}>
      <span class="group-title">{title}</span>
      {#if headerExtra}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="header-extra" onclick={(e) => e.stopPropagation()}>
          {@render headerExtra()}
        </span>
      {/if}
      <svg
        class="chevron"
        class:expanded
        viewBox="0 0 24 24"
        width="12"
        height="12"
        fill="currentColor"
      >
        <path d="M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z"></path>
      </svg>
    </button>
  {/if}
  {#if expanded || collapsed}
    <nav class="group-items" class:hidden={!expanded && !collapsed}>
      {#each items as item (item.href)}
        <a href={item.href} class="nav-item" class:active={isActive(item.href)} title={item.label}>
          <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
            <path d={item.icon}></path>
          </svg>
          {#if !collapsed}
            <span class="nav-label">{item.label}</span>
            {#if item.badge !== undefined && item.badge > 0}
              <span class="nav-badge">{item.badge}</span>
            {:else if item.count !== undefined && item.count > 0}
              <span class="nav-count">{item.count}</span>
            {/if}
          {/if}
        </a>
      {/each}
      {#if children}
        {@render children()}
      {/if}
    </nav>
  {/if}
</div>

<style>
  .nav-group {
    margin-bottom: 4px;
  }

  .group-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 6px 12px;
    background: transparent;
    border: none;
    color: var(--muted);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    cursor: pointer;
    transition: color 0.15s;
  }

  .group-header:hover {
    color: var(--ink);
  }

  .group-header.has-active {
    color: var(--accent);
  }

  .group-title {
    flex: 1;
    text-align: left;
  }

  .header-extra {
    display: flex;
    align-items: center;
  }

  .chevron {
    transition: transform 0.2s;
    opacity: 0.6;
  }

  .chevron.expanded {
    transform: rotate(180deg);
  }

  .collapsed .chevron {
    display: none;
  }

  .group-items {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 8px;
  }

  .group-items.hidden {
    display: none;
  }

  .collapsed .group-items {
    padding: 0 4px;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-radius: 10px;
    color: var(--ink);
    text-decoration: none;
    font-size: 13px;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .nav-item:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  :global([data-theme='light']) .nav-item:hover {
    background: rgba(0, 0, 0, 0.06);
  }

  .nav-item.active {
    background: linear-gradient(90deg, var(--accent-dim) 0%, rgba(211, 140, 90, 0.05) 100%);
    color: var(--accent);
    border-radius: 0 10px 10px 0;
    margin-left: -8px;
    padding-left: 16px;
    border-left: 3px solid var(--accent);
  }

  :global([data-theme='light']) .nav-item.active {
    background: linear-gradient(90deg, var(--accent-dim) 0%, rgba(200, 122, 70, 0.05) 100%);
  }

  .nav-item svg {
    flex-shrink: 0;
    opacity: 0.7;
  }

  .nav-item.active svg {
    opacity: 1;
  }

  .nav-label {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .collapsed .nav-item {
    justify-content: center;
    padding: 10px;
  }

  .nav-count {
    color: var(--muted);
    font-size: 12px;
    margin-left: auto;
  }

  .nav-badge {
    background: var(--accent);
    color: #000;
    padding: 1px 6px;
    border-radius: 10px;
    font-size: 10px;
    font-weight: 700;
    margin-left: auto;
  }
</style>
