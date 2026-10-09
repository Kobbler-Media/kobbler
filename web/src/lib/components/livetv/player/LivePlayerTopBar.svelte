<script lang="ts">
  interface Props {
    lcn: string;
    name: string;
    hdLabel: string;
    tint: string;
    meta: string;
    tunersInUse: number;
    tunerCount: number;
    signalQuality: number | null;
    logoUrl?: string | null;
    onBack: () => void;
  }

  let {
    lcn,
    name,
    hdLabel,
    tint,
    meta,
    tunersInUse,
    tunerCount,
    signalQuality,
    logoUrl = null,
    onBack
  }: Props = $props();

  const bars = $derived.by(() => {
    const q = signalQuality;
    const lit = q == null ? 4 : q >= 95 ? 4 : q >= 85 ? 3 : q >= 70 ? 2 : 1;
    return [1, 2, 3, 4].map((i) => i <= lit);
  });

  const signalTone = $derived(
    signalQuality == null || signalQuality >= 85 ? 'good' : signalQuality >= 70 ? 'fair' : 'poor'
  );
</script>

<div class="top-bar">
  <button type="button" class="back" onclick={onBack} aria-label="Close the player">
    <span class="material-icons-outlined">chevron_left</span>
  </button>

  <div class="chip identity">
    {#if logoUrl}
      <img class="logo" src={logoUrl} alt="" />
    {:else}
      <span class="tint" style:background={tint}></span>
    {/if}
    <span class="lcn">{lcn}</span>
    <span class="name">{name}</span>
    <span class="hd" style:background={tint}>{hdLabel}</span>
  </div>

  <div class="spacer"></div>

  <div class="chip status">
    {#if meta}
      <span class="meta">{meta}</span>
      <span class="sep"></span>
    {/if}
    <span class="meta">{tunersInUse} of {tunerCount} tuners</span>
    <span class="bars" data-tone={signalTone} title="Signal quality">
      {#each bars as lit, i (i)}
        <span class="bar" class:lit style:height="{4 + i * 2.7}px"></span>
      {/each}
    </span>
  </div>
</div>

<style>
  .top-bar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 20px 24px;
    padding-right: calc(24px + env(safe-area-inset-right));
    z-index: 3;
  }

  .back {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.4);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: var(--ink);
    cursor: pointer;
    flex-shrink: 0;
  }

  .back:hover {
    background: rgba(0, 0, 0, 0.62);
  }

  .back .material-icons-outlined {
    font-size: 21px;
  }

  .chip {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 13px;
    border-radius: 11px;
    background: rgba(0, 0, 0, 0.45);
    border: 1px solid rgba(255, 255, 255, 0.09);
    min-width: 0;
  }

  .status {
    gap: 9px;
    flex-shrink: 0;
  }

  .tint {
    width: 3px;
    height: 20px;
    border-radius: 2px;
    flex-shrink: 0;
  }

  .logo {
    width: 22px;
    height: 22px;
    object-fit: contain;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .lcn {
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .name {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hd {
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.05em;
    color: #07080a;
    border-radius: 3px;
    padding: 2px 5px;
    flex-shrink: 0;
  }

  .spacer {
    flex-grow: 1;
  }

  .meta {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
  }

  .sep {
    width: 1px;
    height: 13px;
    background: rgba(255, 255, 255, 0.14);
  }

  .bars {
    display: flex;
    align-items: flex-end;
    gap: 1.5px;
    height: 12px;
  }

  .bar {
    width: 2.5px;
    border-radius: 1px;
    background: rgba(255, 255, 255, 0.2);
  }

  .bars[data-tone='good'] .bar.lit {
    background: var(--success);
  }

  .bars[data-tone='fair'] .bar.lit {
    background: var(--warning);
  }

  .bars[data-tone='poor'] .bar.lit {
    background: var(--danger);
  }

  @media (max-width: 720px) {
    .top-bar {
      padding: 12px 14px;
    }

    .status .meta:first-child,
    .status .sep {
      display: none;
    }
  }
</style>
