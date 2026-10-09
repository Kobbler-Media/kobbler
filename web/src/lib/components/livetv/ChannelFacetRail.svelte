<script lang="ts">
  import type { LivetvFacet, LivetvFacets } from '$lib/types';
  import type { FacetGroupId } from './types';

  interface Props {
    facets: LivetvFacets | null;
    selected: Record<FacetGroupId, string | null>;
    onSelect: (group: FacetGroupId, value: string | null) => void;
    onClear: () => void;
  }

  let { facets, selected, onSelect, onClear }: Props = $props();

  const GROUPS: { id: FacetGroupId; title: string }[] = [
    { id: 'network', title: 'Network' },
    { id: 'region', title: 'Region' },
    { id: 'type', title: 'Type' },
    { id: 'guide', title: 'Guide' },
    { id: 'status', title: 'Status' }
  ];

  const hasAny = $derived(GROUPS.some((g) => selected[g.id] !== null));

  function itemsFor(group: FacetGroupId): LivetvFacet[] {
    return facets?.[group] ?? [];
  }
</script>

<aside class="rail">
  <div class="rail-head">
    <span class="rail-title">Filters</span>
    <button type="button" class="clear" disabled={!hasAny} onclick={onClear}>Clear</button>
  </div>

  {#if !facets}
    <p class="muted empty">No scan data yet.</p>
  {:else}
    <div class="groups">
      {#each GROUPS as group (group.id)}
        {@const items = itemsFor(group.id)}
        {#if items.length > 0}
          <div class="group">
            <span class="group-title">{group.title}</span>
            {#each items as item (item.value)}
              {@const active = selected[group.id] === item.value}
              <button
                type="button"
                class="facet"
                class:active
                aria-pressed={active}
                onclick={() => onSelect(group.id, active ? null : item.value)}
              >
                {#if active}
                  <span class="material-icons-outlined check">check</span>
                {:else}
                  <span class="box"></span>
                {/if}
                <span class="label">{item.label}</span>
                <span class="count">{item.count}</span>
              </button>
            {/each}
          </div>
        {/if}
      {/each}
    </div>
  {/if}
</aside>

<style>
  .rail {
    width: 216px;
    flex-shrink: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    padding: 16px 14px;
    align-self: flex-start;
  }

  .rail-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }

  .rail-title {
    font-size: 13px;
    font-weight: 600;
    flex-grow: 1;
  }

  .clear {
    background: none;
    border: none;
    padding: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    cursor: pointer;
  }

  .clear:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .empty {
    font-size: 12px;
    margin: 0;
  }

  .groups {
    display: flex;
    flex-direction: column;
    gap: 13px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .group-title {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--muted);
    margin-bottom: 3px;
  }

  .facet {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 24px;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
  }

  .box {
    width: 12px;
    height: 12px;
    border-radius: 3px;
    border: 1px solid var(--muted);
    flex-shrink: 0;
  }

  .check {
    font-size: 13px;
    color: var(--accent);
    width: 12px;
    flex-shrink: 0;
  }

  .label {
    font-size: 12px;
    color: var(--muted);
    flex-grow: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .facet.active .label,
  .facet:hover .label {
    color: var(--ink);
  }

  .count {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 900px) {
    .rail {
      width: 100%;
    }
  }
</style>
