<script lang="ts">
  import type { LivetvAdBreak } from '$lib/types';

  interface Props {
    breaks: LivetvAdBreak[];
    posture: 'shade' | 'button' | 'auto';
    onAdjust: (adBreak: LivetvAdBreak) => void;
  }

  let { breaks, posture, onAdjust }: Props = $props();

  function clock(seconds: number): string {
    const m = Math.floor(seconds / 60);
    const s = Math.floor(seconds % 60);
    return `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  }

  function methodLabel(method: string): string {
    if (method === 'fingerprint') return 'FINGERPRINT';
    if (method === 'black_silence') return 'BLACK + SILENCE';
    if (method === 'manual') return 'CORRECTED';
    return method.replace(/_/g, ' ').toUpperCase();
  }

  function methodTone(method: string): 'known' | 'approx' {
    return method === 'fingerprint' || method === 'manual' ? 'known' : 'approx';
  }

  function evidence(b: LivetvAdBreak): string {
    if (b.corrected_at) return 'boundaries corrected by you';
    if (b.ads_total != null && b.ads_known != null) {
      return `${b.ads_known} of ${b.ads_total} ads already known`;
    }
    return 'boundaries approximate';
  }
</script>

<div class="panel">
  <div class="head">
    <span class="heading">How these were found</span>
    <span class="spacer"></span>
    <span class="note">
      adjusting a boundary re-fingerprints the ad, so every future airing of it lands correctly
    </span>
  </div>

  {#if breaks.length === 0}
    <p class="empty">
      No ad breaks marked on this recording. Detection needs two agreeing signals before it marks
      anything — nothing marked beats a wrong mark.
    </p>
  {:else}
    {#each breaks as b, i (b.id)}
      <div class="break">
        <span class="swatch"></span>
        <span class="name">Ad break {i + 1}</span>
        <span class="range">{clock(b.start_seconds)} – {clock(b.end_seconds)}</span>
        <span class="method" data-tone={methodTone(b.method)}>{methodLabel(b.method)}</span>
        <span class="evidence">{evidence(b)}</span>
        <button type="button" class="adjust" onclick={() => onAdjust(b)}>Adjust</button>
      </div>
    {/each}
  {/if}

  <span class="posture">
    {posture === 'auto'
      ? 'Auto-skip is on — every skip is announced so you can rewind it.'
      : 'Auto-skip is off — breaks are marked, you decide.'}
  </span>
</div>

<style>
  .panel {
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: rgba(15, 17, 21, 0.9);
    backdrop-filter: blur(6px);
    padding: 13px 15px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    max-height: 34vh;
    overflow-y: auto;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 11px;
  }

  .heading {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ink);
  }

  .spacer {
    flex-grow: 1;
  }

  .note,
  .posture {
    font-size: 11px;
    color: #5f6874;
  }

  .empty {
    margin: 0;
    font-size: 11.5px;
    color: var(--muted);
    line-height: 1.5;
  }

  .break {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 9px 12px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid #23272e;
  }

  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--danger);
    flex-shrink: 0;
  }

  .name {
    width: 86px;
    flex-shrink: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
  }

  .range {
    width: 104px;
    flex-shrink: 0;
    font-size: 11.5px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .method {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.04em;
    border-radius: 4px;
    padding: 2px 7px;
    flex-shrink: 0;
  }

  .method[data-tone='known'] {
    color: var(--success);
    border: 1px solid var(--success);
  }

  .method[data-tone='approx'] {
    color: var(--warning);
    border: 1px solid var(--warning);
  }

  .evidence {
    font-size: 11px;
    color: #5f6874;
    flex-grow: 1;
  }

  .adjust {
    font-size: 11px;
    color: var(--accent);
    font-weight: 600;
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
  }

  .adjust:hover {
    text-decoration: underline;
  }

  :global([data-theme='light']) .panel {
    background: rgba(255, 255, 255, 0.92);
  }

  :global([data-theme='light']) .break {
    background: rgba(0, 0, 0, 0.03);
    border-color: var(--ring);
  }
</style>
