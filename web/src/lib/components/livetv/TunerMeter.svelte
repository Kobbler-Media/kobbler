<script lang="ts">
  import type { TunerLease } from './types';

  interface Props {
    tunerCount: number;
    leasesInUse: number;
    leases?: TunerLease[];
    compact?: boolean;
    note?: string;
  }

  let { tunerCount, leasesInUse, leases = [], compact = false, note }: Props = $props();

  const slots = $derived(
    Array.from(
      { length: Math.max(tunerCount, 0) },
      (_, i) => leases[i] ?? (i < leasesInUse ? null : undefined)
    )
  );
</script>

{#if compact}
  <span class="pips" title={`${leasesInUse} of ${tunerCount} tuners in use`}>
    {#each slots as slot, i (i)}
      <span class="pip" class:busy={slot !== undefined} class:recording={slot?.kind === 'recording'}
      ></span>
    {/each}
    <span class="count">{leasesInUse} / {tunerCount}</span>
  </span>
{:else}
  <div class="meter">
    <div class="meter-head">
      <span class="title">Tuners</span>
      <span class="count">{leasesInUse} / {tunerCount}</span>
    </div>
    <div class="pips">
      {#each slots as slot, i (i)}
        <span
          class="pip wide"
          class:busy={slot !== undefined}
          class:recording={slot?.kind === 'recording'}
        ></span>
      {/each}
    </div>
    {#if leases.length > 0}
      <div class="leases">
        {#each leases as lease (lease.id)}
          <div class="lease">
            <span class="dot" class:recording={lease.kind === 'recording'}></span>
            <span class="lease-kind">{lease.kind === 'recording' ? 'Recording' : 'Watching'}</span>
            <span class="lease-label">{lease.label}</span>
          </div>
        {/each}
      </div>
    {/if}
    {#if note}
      <span class="note">{note}</span>
    {/if}
  </div>
{/if}

<style>
  .pips {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .pip {
    width: 16px;
    height: 5px;
    border-radius: 2px;
    background: var(--ring);
    flex-shrink: 0;
  }

  .pip.wide {
    width: 26px;
    height: 6px;
    border-radius: 3px;
  }

  .pip.busy {
    background: var(--accent);
  }

  .pip.recording {
    background: var(--danger);
  }

  .count {
    font-size: 11px;
    font-weight: 600;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
    margin-left: 4px;
  }

  .meter {
    display: flex;
    flex-direction: column;
    gap: 9px;
  }

  .meter-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .title {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .meter .count {
    margin-left: 0;
  }

  .leases {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .lease {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted);
    min-width: 0;
  }

  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
    flex-shrink: 0;
  }

  .dot.recording {
    background: var(--danger);
  }

  .lease-kind {
    color: var(--ink);
  }

  .lease-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.5;
  }
</style>
