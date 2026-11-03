<script lang="ts">
  import type { LivetvLease, LivetvRecording, LivetvTuner } from '#lib/types.js';

  interface Props {
    tuners: LivetvTuner[];
    leases: LivetvLease[];
    recordings: LivetvRecording[];
    windowHours?: number;
  }

  let { tuners, leases, recordings, windowHours = 6 }: Props = $props();

  type Band = {
    id: string;
    lane: number;
    leftPct: number;
    widthPct: number;
    kind: 'recording' | 'watching' | 'moved';
    label: string;
  };

  const laneCount = $derived(
    Math.max(
      1,
      tuners.reduce((n, t) => n + (t.tuner_count ?? 0), 0)
    )
  );

  let now = $state(Date.now());

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  const windowStart = $derived(now - (windowHours - 2) * 3600_000);
  const windowEnd = $derived(windowStart + windowHours * 3600_000);
  const span = $derived(windowEnd - windowStart);

  const hourTicks = $derived.by(() => {
    const ticks: { label: string; pct: number }[] = [];
    const first = new Date(windowStart);
    first.setMinutes(0, 0, 0);
    for (let t = first.getTime(); t <= windowEnd; t += 3600_000) {
      if (t < windowStart) continue;
      ticks.push({
        label: new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
        pct: ((t - windowStart) / span) * 100
      });
    }
    return ticks;
  });

  const nowPct = $derived(((now - windowStart) / span) * 100);

  function clampBand(startMs: number, endMs: number): { left: number; width: number } | null {
    const from = Math.max(startMs, windowStart);
    const to = Math.min(endMs, windowEnd);
    if (to <= from) return null;
    return { left: ((from - windowStart) / span) * 100, width: ((to - from) / span) * 100 };
  }

  const bands = $derived.by(() => {
    const out: Band[] = [];
    const laneOf = new Map<string, number>();
    let nextLane = 0;
    const assign = (key: string, preferred: number | null): number => {
      if (preferred != null && preferred >= 0 && preferred < laneCount) return preferred;
      const existing = laneOf.get(key);
      if (existing != null) return existing;
      const lane = nextLane % laneCount;
      nextLane += 1;
      laneOf.set(key, lane);
      return lane;
    };

    for (const rec of recordings) {
      if (rec.status !== 'recording' && rec.status !== 'scheduled') continue;
      const start = Date.parse(rec.actual_start ?? rec.scheduled_start);
      const end = Date.parse(rec.scheduled_end);
      if (!isFinite(start) || !isFinite(end)) continue;
      const box = clampBand(start, end);
      if (!box) continue;
      out.push({
        id: `rec-${rec.id}`,
        lane: assign(`rec-${rec.id}`, rec.tuner_index),
        leftPct: box.left,
        widthPct: box.width,
        kind: rec.moved_to_guide_id ? 'moved' : 'recording',
        label: rec.moved_to_guide_id
          ? `REC ${rec.title} · moved from a clash`
          : `REC ${rec.title} · ${rec.channel_name}`
      });
    }

    for (const lease of leases) {
      if (lease.recording_id) continue;
      const start = Date.parse(lease.acquired_at);
      if (!isFinite(start)) continue;
      const end = lease.expires_at ? Date.parse(lease.expires_at) : now;
      const box = clampBand(start, Math.max(end, now));
      if (!box) continue;
      out.push({
        id: `lease-${lease.id}`,
        lane: assign(`lease-${lease.id}`, lease.tuner_index),
        leftPct: box.left,
        widthPct: box.width,
        kind: 'watching',
        label: `${lease.channel_name} · you`
      });
    }
    return out;
  });

  const inUse = $derived(leases.length);

  const clashNote = $derived.by(() => recordings.find((r) => r.moved_to_guide_id) ?? null);

  function bandsFor(lane: number) {
    return bands.filter((b) => b.lane === lane);
  }
</script>

<section class="timeline">
  <header>
    <span class="title">Tuner leases</span>
    <span class="sub">{inUse} of {laneCount} in use</span>
    <span class="spacer"></span>
    <div class="legend">
      <span class="key"><span class="swatch recording"></span>recording</span>
      <span class="key"><span class="swatch watching"></span>watching</span>
      <span class="key"><span class="swatch moved"></span>moved by a clash</span>
    </div>
  </header>

  <div class="axis">
    <span class="gutter"></span>
    <div class="axis-track">
      {#each hourTicks as tick (tick.pct)}
        <span class="tick" style:left="{tick.pct}%">{tick.label}</span>
      {/each}
    </div>
  </div>

  <div class="lanes">
    <div class="now-line" style:left="calc(64px + (100% - 64px) * {nowPct / 100})"></div>
    {#each Array.from({ length: laneCount }, (_, i) => i) as lane (lane)}
      <div class="lane">
        <span class="gutter">tuner{lane}</span>
        <div class="lane-track">
          {#each bandsFor(lane) as band (band.id)}
            <div
              class="band {band.kind}"
              style:left="{band.leftPct}%"
              style:width="{band.widthPct}%"
              title={band.label}
            >
              <span>{band.label}</span>
            </div>
          {/each}
          {#if bandsFor(lane).length === 0}
            <span class="free">free</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  {#if clashNote}
    <div class="clash">
      <span class="material-icons-outlined">history</span>
      <span>
        <strong>{clashNote.title}</strong> clashed with every tuner busy, so the rule took a later airing
        instead of dropping the episode.
      </span>
    </div>
  {/if}
</section>

<style>
  .timeline {
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    padding: 14px 16px 8px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 10px;
  }

  .title {
    font-size: 13px;
    font-weight: 600;
  }

  .sub {
    font-size: 11.5px;
    color: var(--muted);
  }

  .spacer {
    flex-grow: 1;
  }

  .legend {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .key {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted);
  }

  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
  }

  .swatch.recording {
    background: rgba(239, 68, 68, 0.16);
    border: 1px solid var(--danger);
  }

  .swatch.watching {
    background: var(--accent-dim);
    border: 1px solid var(--accent);
  }

  .swatch.moved {
    background: rgba(139, 92, 246, 0.16);
    border: 1px solid #8b5cf6;
  }

  .axis {
    display: flex;
    margin-bottom: 2px;
  }

  .gutter {
    width: 64px;
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    font-family: ui-monospace, monospace;
  }

  .axis-track {
    position: relative;
    flex-grow: 1;
    height: 14px;
  }

  .tick {
    position: absolute;
    top: 0;
    font-size: 10.5px;
    color: #5f6874;
    font-variant-numeric: tabular-nums;
    padding-left: 5px;
    border-left: 1px solid var(--ring);
  }

  .lanes {
    position: relative;
  }

  .now-line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--accent);
    z-index: 3;
    box-shadow: 0 0 10px rgba(211, 140, 90, 0.6);
  }

  .lane {
    display: flex;
    align-items: center;
    height: 44px;
    border-bottom: 1px solid var(--ring);
  }

  .lane-track {
    position: relative;
    flex-grow: 1;
    height: 44px;
  }

  .band {
    position: absolute;
    top: 5px;
    height: 34px;
    box-sizing: border-box;
    border-radius: 7px;
    display: flex;
    align-items: center;
    padding: 0 9px;
    overflow: hidden;
  }

  .band span {
    font-size: 11px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .band.recording {
    border: 1px solid var(--danger);
    background: rgba(239, 68, 68, 0.16);
  }

  .band.watching {
    border: 1px solid var(--accent);
    background: var(--accent-dim);
  }

  .band.moved {
    border: 1px solid #8b5cf6;
    background: rgba(139, 92, 246, 0.16);
  }

  .free {
    position: absolute;
    left: 12px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 11px;
    color: #3f4753;
  }

  .clash {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 0 4px;
    font-size: 11.5px;
    color: var(--muted);
  }

  .clash strong {
    color: var(--ink);
  }

  .clash .material-icons-outlined {
    font-size: 15px;
    color: #8b5cf6;
    flex-shrink: 0;
  }

  :global([data-theme='light']) .band.recording {
    background: rgba(239, 68, 68, 0.1);
  }

  :global([data-theme='light']) .band.moved {
    background: rgba(139, 92, 246, 0.1);
  }
</style>
