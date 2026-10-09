<script lang="ts">
  /**
   * UpgradeStatusBadge - Shows when an item has an active upgrade window.
   *
   * TODO: Wire data source — the upgradeUntil date should come from the entity's
   * watch pattern status (kobload pattern_has_active_download + upgrade_until).
   * For now this component accepts the date as a prop and the parent is
   * responsible for fetching it.
   */

  interface Props {
    /** ISO date string for when the upgrade window expires. If null, badge is hidden. */
    upgradeUntil?: string | null;
  }

  let { upgradeUntil = null }: Props = $props();

  const label = $derived.by(() => {
    if (!upgradeUntil) return null;
    try {
      const date = new Date(upgradeUntil);
      const now = new Date();
      if (date <= now) return null;
      return `Upgrading until ${date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`;
    } catch {
      return 'Looking for better quality';
    }
  });
</script>

{#if label}
  <div class="upgrade-badge">
    <span class="material-icons-outlined">upgrade</span>
    {label}
  </div>
{/if}

<style>
  .upgrade-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    color: var(--accent);
    font-size: 11px;
    font-weight: 700;
    white-space: nowrap;
  }

  .upgrade-badge .material-icons-outlined {
    font-size: 14px;
  }
</style>
