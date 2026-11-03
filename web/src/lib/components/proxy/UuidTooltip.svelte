<script lang="ts" module>
  const _resolveCache = new Map<string, ResolvedEntity | null>();

  function getResolveCache(): Map<string, ResolvedEntity | null> {
    return _resolveCache;
  }
</script>

<script lang="ts">
  import { resolveUuids, type EntityRef, type ResolvedEntity } from '#lib/api/client.js';
  import { openCommandPalette, setCommandPaletteQuery } from '#lib/stores/ui.svelte.js';

  interface Props {
    uuid: string;
    entityRefs?: EntityRef[];
  }

  let { uuid, entityRefs = [] }: Props = $props();

  // Module-level cache shared across all instances
  const resolveCache = getResolveCache();

  let showTooltip = $state(false);
  let resolved = $state<ResolvedEntity | null | undefined>(undefined);
  let loading = $state(false);

  const entityFromRefs = $derived(entityRefs.find((r) => r.uuid === uuid));

  const isOurEntity = $derived(resolved !== null && resolved !== undefined);

  const displayLabel = $derived.by(() => {
    if (entityFromRefs) {
      const type =
        entityFromRefs.entity_type.charAt(0).toUpperCase() + entityFromRefs.entity_type.slice(1);
      return `${type}: ${entityFromRefs.name}`;
    }
    if (resolved) {
      const type = resolved.entity_type.charAt(0).toUpperCase() + resolved.entity_type.slice(1);
      return `${type}: ${resolved.name}`;
    }
    if (resolved === null) {
      return 'External (Kobo)';
    }
    if (loading) {
      return 'Loading...';
    }
    return null;
  });

  async function handleMouseEnter() {
    showTooltip = true;

    // Already have info from entity_refs
    if (entityFromRefs) return;

    // Check cache
    if (resolveCache.has(uuid)) {
      resolved = resolveCache.get(uuid) ?? null;
      return;
    }

    // Fetch from API
    if (loading) return;
    loading = true;
    try {
      const result = await resolveUuids([uuid]);
      const entity = result.entities[uuid] ?? null;
      resolveCache.set(uuid, entity);
      resolved = entity;
    } catch {
      resolved = null;
    } finally {
      loading = false;
    }
  }

  function handleMouseLeave() {
    showTooltip = false;
  }

  function handleClick(event: MouseEvent) {
    event.stopPropagation();
    if (entityFromRefs || isOurEntity) {
      setCommandPaletteQuery(uuid);
      openCommandPalette();
    } else {
      navigator.clipboard.writeText(uuid);
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      handleClick(event as unknown as MouseEvent);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions --><span
  class="uuid-tooltip-wrapper"
  onmouseenter={handleMouseEnter}
  onmouseleave={handleMouseLeave}
  ><span
    class="uuid-link"
    class:uuid-external={!entityFromRefs && resolved === null && !loading}
    data-uuid={uuid}
    role="button"
    tabindex="0"
    onclick={handleClick}
    onkeydown={handleKeydown}>{uuid}</span
  >{#if showTooltip && displayLabel}<span class="uuid-tooltip">{displayLabel}</span>{/if}</span
>

<style>
  .uuid-tooltip-wrapper {
    position: relative;
    display: inline;
  }

  .uuid-link {
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
    text-decoration-style: dotted;
  }

  .uuid-link:hover {
    text-decoration: none;
  }

  .uuid-link.uuid-external {
    color: var(--muted);
    text-decoration-style: dashed;
  }

  .uuid-tooltip {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 4px 10px;
    font-size: 11px;
    white-space: nowrap;
    z-index: 100;
    pointer-events: none;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
    color: var(--ink);
  }
</style>
