<script lang="ts">
  import type { EntityRef } from '$lib/api/client';

  interface Props {
    notes: string | null;
    entityRefs: EntityRef[];
    alertMessage: string | null;
  }

  let { notes, entityRefs, alertMessage }: Props = $props();

  // Group entity refs by source
  const groupedRefs = $derived.by(() => {
    const groups = new Map<string, EntityRef[]>();
    for (const ref of entityRefs) {
      const existing = groups.get(ref.source) ?? [];
      // Deduplicate by uuid within source
      if (!existing.some((r) => r.uuid === ref.uuid)) {
        existing.push(ref);
      }
      groups.set(ref.source, existing);
    }
    return groups;
  });

  const sourceLabels: Record<string, string> = {
    path: 'Path',
    request_body: 'Request Body',
    response_body: 'Response Body',
    upstream_body: 'Upstream Body'
  };

  const entityTypeIcons: Record<string, string> = {
    book: 'B',
    shelf: 'S',
    series: 'Sr',
    author: 'A',
    device: 'D'
  };

  function entityUrl(ref: EntityRef): string | null {
    switch (ref.entity_type) {
      case 'book':
        return `/library/books/${ref.uuid}`;
      case 'author':
        return `/authors/${ref.uuid}`;
      case 'series':
        return `/series/${ref.uuid}`;
      default:
        return null;
    }
  }
</script>

{#if alertMessage}
  <div class="alert-warning">{alertMessage}</div>
{/if}

{#if notes}
  <p class="plain-notes">{notes}</p>
{/if}

{#if groupedRefs.size > 0}
  <div class="entity-refs">
    {#each [...groupedRefs.entries()] as [source, refs] (source)}
      <div class="ref-group">
        <span class="ref-source">{sourceLabels[source] ?? source}</span>
        <div class="ref-items">
          {#each refs as ref (ref.uuid)}
            <span class="ref-item">
              <span class="ref-type-badge" title={ref.entity_type}>
                {entityTypeIcons[ref.entity_type] ?? '?'}
              </span>
              {#if entityUrl(ref)}
                <a href={entityUrl(ref)} class="ref-name">{ref.name}</a>
              {:else}
                <span class="ref-name">{ref.name}</span>
              {/if}
            </span>
          {/each}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .alert-warning {
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid rgba(239, 68, 68, 0.4);
    color: #ef4444;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 12px;
    font-weight: 600;
    margin-bottom: 8px;
  }

  .entity-refs {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .ref-group {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }

  .ref-source {
    font-size: 11px;
    color: var(--muted);
    min-width: 100px;
    flex-shrink: 0;
    padding-top: 2px;
  }

  .ref-items {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .ref-item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: var(--bg);
    padding: 2px 8px;
    border-radius: 6px;
    font-size: 12px;
  }

  .ref-type-badge {
    font-size: 10px;
    font-weight: 700;
    color: var(--accent);
    min-width: 14px;
    text-align: center;
  }

  .ref-name {
    color: var(--ink);
    text-decoration: none;
  }

  a.ref-name:hover {
    text-decoration: underline;
    color: var(--accent);
  }

  .plain-notes {
    margin: 0;
    font-size: 13px;
  }
</style>
