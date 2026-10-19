<script lang="ts" module>
  export interface MenuItem {
    label: string;
    icon?: string;
    onSelect: () => void;
    danger?: boolean;
    disabled?: boolean;
    checked?: boolean;
    section?: string;
    title?: string;
  }

  export type MenuEntry = MenuItem | false | null | undefined;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { portal } from '$lib/utils/portal';
  import { uid } from '@koblibs/ui';

  interface Props {
    items: MenuEntry[];
    label?: string;
    text?: string;
    icon?: string;
    busy?: boolean;
    variant?: 'pill' | 'icon';
  }

  let {
    items,
    label = 'More actions',
    text,
    icon = 'more_horiz',
    busy = false,
    variant = 'pill'
  }: Props = $props();

  const menuId = `overflow-menu-${uid()}`;
  const visible = $derived(items.filter((i): i is MenuItem => !!i));

  let open = $state(false);
  let triggerEl = $state<HTMLButtonElement>();
  let menuEl = $state<HTMLDivElement>();
  let position = $state<{ top: number; left: number } | null>(null);

  function enabledItems(): HTMLButtonElement[] {
    return menuEl
      ? [...menuEl.querySelectorAll<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)')]
      : [];
  }

  function place() {
    if (!triggerEl || !menuEl) return;
    const t = triggerEl.getBoundingClientRect();
    const m = menuEl.getBoundingClientRect();
    const margin = 8;
    const left = Math.min(
      Math.max(margin, t.right - m.width),
      window.innerWidth - m.width - margin
    );
    const below = t.bottom + 4;
    const above = t.top - 4 - m.height;
    const top = below + m.height <= window.innerHeight - margin || above < margin ? below : above;
    position = { top, left };
  }

  async function show(focus: 'first' | 'last' = 'first') {
    open = true;
    position = null;
    await tick();
    place();
    await tick();
    const els = enabledItems();
    (focus === 'first' ? els[0] : els[els.length - 1])?.focus();
  }

  function hide(restoreFocus = true) {
    if (!open) return;
    open = false;
    if (restoreFocus) triggerEl?.focus();
  }

  function select(item: MenuItem) {
    hide();
    item.onSelect();
  }

  function onTriggerKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      show(e.key === 'ArrowDown' ? 'first' : 'last');
    }
  }

  function onMenuKeydown(e: KeyboardEvent) {
    const els = enabledItems();
    const at = els.indexOf(document.activeElement as HTMLButtonElement);
    const focusAt = (i: number) => els[(i + els.length) % els.length]?.focus();
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        focusAt(at + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        focusAt(at - 1);
        break;
      case 'Home':
        e.preventDefault();
        focusAt(0);
        break;
      case 'End':
        e.preventDefault();
        focusAt(els.length - 1);
        break;
      case 'Escape':
        e.preventDefault();
        e.stopPropagation();
        hide();
        break;
      case 'Tab':
        hide(false);
        break;
    }
  }

  $effect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!menuEl?.contains(target) && !triggerEl?.contains(target)) hide(false);
    };
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') hide();
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    document.addEventListener('keydown', onKeydown);
    window.addEventListener('scroll', place, true);
    window.addEventListener('resize', place);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true);
      document.removeEventListener('keydown', onKeydown);
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('resize', place);
    };
  });
</script>

{#if visible.length > 0}
  <button
    bind:this={triggerEl}
    type="button"
    class="overflow-trigger {variant}"
    class:with-text={!!text}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? menuId : undefined}
    aria-label={text ? undefined : label}
    title={label}
    disabled={busy}
    onclick={(e) => {
      e.stopPropagation();
      if (open) hide(false);
      else show();
    }}
    onkeydown={onTriggerKeydown}
  >
    <span class="material-icons-outlined" aria-hidden="true">{busy ? 'hourglass_empty' : icon}</span
    >
    {#if text}<span class="trigger-text">{text}</span>{/if}
  </button>
{/if}

{#if open}
  <div
    bind:this={menuEl}
    id={menuId}
    class="overflow-menu"
    role="menu"
    aria-label={label}
    tabindex="-1"
    style={position
      ? `top: ${position.top}px; left: ${position.left}px;`
      : 'top: 0; left: 0; visibility: hidden;'}
    use:portal
    onkeydown={onMenuKeydown}
  >
    {#each visible as item, i (i)}
      {#if item.section && item.section !== visible[i - 1]?.section}
        <div class="menu-section" role="presentation">{item.section}</div>
      {:else if i > 0 && !item.section && visible[i - 1]?.section}
        <div class="menu-divider" role="separator"></div>
      {/if}
      <button
        type="button"
        class="menu-item"
        class:danger={item.danger}
        role={item.checked === undefined ? 'menuitem' : 'menuitemradio'}
        aria-checked={item.checked}
        title={item.title}
        disabled={item.disabled}
        tabindex="-1"
        onclick={(e) => {
          e.stopPropagation();
          select(item);
        }}
      >
        {#if item.checked !== undefined}
          <span class="material-icons-outlined" aria-hidden="true">
            {item.checked ? 'radio_button_checked' : 'radio_button_unchecked'}
          </span>
        {:else if item.icon}
          <span class="material-icons-outlined" aria-hidden="true">{item.icon}</span>
        {/if}
        <span class="menu-label">{item.label}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .overflow-trigger {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    flex-shrink: 0;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    font: inherit;
    transition:
      background 0.15s,
      color 0.15s,
      border-color 0.15s;
  }

  .overflow-trigger:hover:not(:disabled),
  .overflow-trigger[aria-expanded='true'] {
    border-color: var(--accent);
    color: var(--ink);
  }

  .overflow-trigger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .overflow-trigger.pill {
    height: 42px;
    min-width: 42px;
    padding: 0 10px;
    border-radius: 100px;
  }

  .overflow-trigger.pill.with-text {
    padding: 0 16px;
    font-size: 14px;
    font-weight: 700;
  }

  .overflow-trigger.icon {
    height: 28px;
    min-width: 28px;
    padding: 0 6px;
    border-radius: 6px;
    background: transparent;
    font-size: 12px;
    font-weight: 600;
  }

  .overflow-trigger.icon .material-icons-outlined {
    font-size: 16px;
  }

  .overflow-menu {
    position: fixed;
    z-index: 1000;
    min-width: 200px;
    max-width: min(320px, calc(100vw - 16px));
    max-height: min(420px, calc(100vh - 16px));
    overflow-y: auto;
    padding: 4px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
    outline: none;
  }

  .menu-section {
    padding: 8px 10px 4px;
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .menu-section:not(:first-child) {
    margin-top: 4px;
    border-top: 1px solid var(--ring);
  }

  .menu-divider {
    height: 1px;
    margin: 4px 0;
    background: var(--ring);
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 40px;
    padding: 8px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--ink);
    font: inherit;
    font-size: 14px;
    font-weight: 500;
    text-align: left;
    cursor: pointer;
  }

  .menu-item .material-icons-outlined {
    font-size: 18px;
    color: var(--muted);
  }

  .menu-item:hover:not(:disabled),
  .menu-item:focus-visible {
    background: var(--bg-2);
    outline: none;
  }

  .menu-item[aria-checked='true'] .material-icons-outlined {
    color: var(--accent);
  }

  .menu-item:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .menu-item.danger,
  .menu-item.danger .material-icons-outlined {
    color: var(--danger);
  }

  .menu-item.danger:hover:not(:disabled),
  .menu-item.danger:focus-visible {
    background: rgba(239, 68, 68, 0.1);
  }
</style>
