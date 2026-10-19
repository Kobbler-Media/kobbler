<script lang="ts" module>
  export interface HeroAction {
    label: string;
    icon: string;
    onSelect: () => void;
    title?: string;
    active?: boolean;
    pinned?: boolean;
    tone?: 'primary' | 'danger';
    iconOnly?: boolean;
    busy?: boolean;
    disabled?: boolean;
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import OverflowMenu, { type MenuEntry, type MenuItem } from './OverflowMenu.svelte';

  interface Props {
    actions: (HeroAction | false | null | undefined)[];
    primary?: Snippet;
    menuItems?: MenuEntry[];
    menuLabel?: string;
  }

  let { actions, primary, menuItems = [], menuLabel = 'More actions' }: Props = $props();

  const list = $derived(actions.filter((a): a is HeroAction => !!a));
  const collapsible = $derived(list.filter((a) => !a.pinned));
  const extraItems = $derived(menuItems.filter((i): i is MenuItem => !!i));

  let bar = $state<HTMLDivElement>();
  let triggerSlot = $state<HTMLSpanElement>();
  let shownCount = $state(Infinity);

  const overflowItems = $derived<MenuItem[]>([
    ...collapsible.slice(shownCount).map((a) => ({
      label: a.label,
      icon: a.icon,
      onSelect: a.onSelect,
      danger: a.tone === 'danger',
      disabled: a.disabled || a.busy,
      title: a.title
    })),
    ...extraItems
  ]);

  function lineCount(): number {
    let lines = 0;
    let lineBottom = -Infinity;
    for (const child of bar!.children) {
      const r = child.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      if (r.top >= lineBottom - 1) {
        lines++;
        lineBottom = r.bottom;
      } else {
        lineBottom = Math.max(lineBottom, r.bottom);
      }
    }
    return lines;
  }

  // Priority+: pinned actions always show; the rest are revealed in order while they fit on the
  // rows the pinned ones already need, and whatever does not fit goes into the overflow menu.
  function fit() {
    if (!bar || !triggerSlot) return;
    const items = [...bar.querySelectorAll<HTMLElement>('[data-collapsible]')];
    triggerSlot.hidden = false;
    triggerSlot.style.minWidth = '42px';
    for (const el of items) el.hidden = true;
    const base = lineCount();
    let shown = 0;
    for (const el of items) {
      el.hidden = false;
      if (lineCount() > base) {
        el.hidden = true;
        break;
      }
      shown++;
    }
    for (const el of items.slice(shown)) el.hidden = true;
    triggerSlot.style.minWidth = '';
    triggerSlot.hidden = shown === items.length && extraItems.length === 0;
    shownCount = shown;
  }

  $effect(() => {
    void list.map((a) => a.label + a.active + a.busy).join();
    void extraItems.length;
    fit();
  });

  $effect(() => {
    if (!bar) return;
    const observer = new ResizeObserver(() => fit());
    observer.observe(bar);
    document.fonts?.ready.then(fit);
    return () => observer.disconnect();
  });
</script>

<div class="hero-action-bar" bind:this={bar}>
  {#if primary}{@render primary()}{/if}
  {#each list as action, i (i)}
    <button
      type="button"
      class="hero-btn"
      class:active={action.active}
      class:primary={action.tone === 'primary'}
      class:danger={action.tone === 'danger'}
      class:icon-only={action.iconOnly}
      data-collapsible={action.pinned ? undefined : ''}
      title={action.title ?? (action.iconOnly ? action.label : undefined)}
      aria-label={action.iconOnly ? action.label : undefined}
      aria-pressed={action.active === undefined ? undefined : action.active}
      disabled={action.disabled || action.busy}
      onclick={action.onSelect}
    >
      <span class="material-icons-outlined" aria-hidden="true">
        {action.busy ? 'hourglass_empty' : action.icon}
      </span>
      {#if !action.iconOnly}<span class="hero-btn-label">{action.label}</span>{/if}
    </button>
  {/each}
  <span class="trigger-slot" bind:this={triggerSlot}>
    <OverflowMenu items={overflowItems} label={menuLabel} />
  </span>
</div>

<style>
  .hero-action-bar {
    flex: 1 1 100%;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: inherit;
    gap: 12px;
    min-width: 0;
  }

  .trigger-slot {
    display: inline-flex;
  }

  .hero-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 42px;
    padding: 0 20px;
    border-radius: 100px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--muted);
    font: inherit;
    font-size: 14px;
    font-weight: 700;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background 0.2s,
      color 0.2s,
      border-color 0.2s,
      transform 0.2s;
  }

  .hero-action-bar > :global([hidden]) {
    display: none;
  }

  .hero-btn .material-icons-outlined {
    font-size: 20px;
  }

  .hero-btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--ink);
    transform: translateY(-1px);
  }

  .hero-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .hero-btn.active {
    background: var(--accent-dim);
    border-color: var(--accent);
    color: var(--accent);
  }

  .hero-btn.primary {
    background: var(--accent);
    border-color: transparent;
    color: #1d130c;
  }

  .hero-btn.danger:hover:not(:disabled) {
    border-color: var(--danger);
    color: var(--danger);
  }

  .hero-btn.icon-only {
    width: 42px;
    padding: 0;
    justify-content: center;
  }

  @media (max-width: 768px) {
    .hero-action-bar {
      gap: 8px;
    }

    .hero-btn {
      height: 40px;
      padding: 0 14px;
      font-size: 13px;
      gap: 6px;
    }

    .hero-btn .material-icons-outlined {
      font-size: 18px;
    }

    .hero-btn.icon-only {
      width: 40px;
    }
  }
</style>
