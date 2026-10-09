<script lang="ts">
  interface Props {
    trackStart: number;
    trackEnd: number;
    availableStart: number;
    liveEdge: number;
    playhead: number;
    startLabel: string;
    endLabel: string;
    atLive: boolean;
    onSeek: (bufferSeconds: number) => void;
    onInteraction?: () => void;
  }

  let {
    trackStart,
    trackEnd,
    availableStart,
    liveEdge,
    playhead,
    startLabel,
    endLabel,
    atLive,
    onSeek,
    onInteraction
  }: Props = $props();

  let scrubbing = $state(false);
  let scrubValue = $state(0);

  const span = $derived(Math.max(trackEnd - trackStart, 1));
  const head = $derived(scrubbing ? scrubValue : playhead);

  function pct(value: number): number {
    return Math.min(100, Math.max(0, ((value - trackStart) / span) * 100));
  }

  const availPct = $derived(pct(availableStart));
  const edgePct = $derived(pct(liveEdge));
  const headPct = $derived(pct(head));

  function clamp(value: number): number {
    return Math.min(liveEdge, Math.max(availableStart, value));
  }

  function handleInput(e: Event) {
    onInteraction?.();
    scrubbing = true;
    scrubValue = clamp(Number((e.currentTarget as HTMLInputElement).value));
  }

  function handleCommit() {
    scrubbing = false;
    onSeek(scrubValue);
  }
</script>

<div class="seek-row">
  <span class="edge-label">{startLabel}</span>

  <div class="seek" class:scrubbing>
    <div class="track">
      <div
        class="available"
        style="left: {availPct}%; width: {Math.max(edgePct - availPct, 0)}%"
      ></div>
      <div class="progress" style="width: {headPct}%"></div>
      {#if availPct > 0.5}
        <div class="unavailable" style="width: {availPct}%" title="Before you tuned in"></div>
      {/if}
      <div class="thumb" style="left: {headPct}%"></div>
    </div>
    <input
      type="range"
      class="seek-input"
      min={trackStart}
      max={trackEnd}
      step="1"
      value={head}
      oninput={handleInput}
      onchange={handleCommit}
      aria-label="Seek within the live buffer"
    />
  </div>

  <span class="badge" class:live={atLive}>{atLive ? 'LIVE' : 'BEHIND'}</span>
  <span class="edge-label right">{endLabel}</span>
</div>

<style>
  .seek-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .edge-label {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    width: 38px;
    flex-shrink: 0;
  }

  .edge-label.right {
    text-align: right;
  }

  .seek {
    position: relative;
    flex-grow: 1;
    height: 18px;
    display: flex;
    align-items: center;
  }

  .track {
    position: relative;
    width: 100%;
    height: 5px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }

  :global([data-theme='light']) .track {
    background: rgba(0, 0, 0, 0.18);
  }

  .available,
  .progress,
  .unavailable {
    position: absolute;
    top: 0;
    bottom: 0;
    border-radius: 3px;
  }

  .available {
    background: var(--accent-dim);
  }

  .progress {
    left: 0;
    background: var(--accent);
  }

  /* The slice of the programme that aired before we tuned: elapsed, but not on
     disk, so it cannot be scrubbed to. */
  .unavailable {
    left: 0;
    background: repeating-linear-gradient(
      -45deg,
      rgba(0, 0, 0, 0.55) 0 3px,
      rgba(0, 0, 0, 0.2) 3px 6px
    );
  }

  .thumb {
    position: absolute;
    top: 50%;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid #07080a;
    transform: translate(-50%, -50%);
    pointer-events: none;
  }

  .seek-input {
    position: absolute;
    inset: 0;
    width: 100%;
    margin: 0;
    opacity: 0;
    cursor: pointer;
    appearance: none;
    background: transparent;
  }

  .badge {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    border-radius: 4px;
    padding: 3px 7px;
    flex-shrink: 0;
    color: var(--muted);
    background: transparent;
    border: 1px solid var(--ring);
  }

  .badge.live {
    color: #07080a;
    background: var(--accent);
    border-color: var(--accent);
  }

  @media (max-width: 720px) {
    .edge-label {
      width: 32px;
      font-size: 10px;
    }
  }
</style>
