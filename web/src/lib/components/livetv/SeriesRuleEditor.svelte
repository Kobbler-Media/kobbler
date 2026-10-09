<script lang="ts">
  import type { LivetvChannel, LivetvClashAction, LivetvSeriesRuleInput } from '$lib/types';

  interface Props {
    value: LivetvSeriesRuleInput;
    channels: LivetvChannel[];
    onChange: (next: LivetvSeriesRuleInput) => void;
  }

  let { value, channels, onChange }: Props = $props();

  const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const CLASH: { id: LivetvClashAction; label: string }[] = [
    { id: 'move', label: 'Move to a later airing' },
    { id: 'drop', label: 'Drop' },
    { id: 'ask', label: 'Ask' }
  ];

  const episodeMode = $derived(
    value.min_season != null ? 'season' : value.new_only ? 'new' : 'all'
  );

  function patch(fields: Partial<LivetvSeriesRuleInput>) {
    onChange({ ...value, ...fields });
  }

  function toggleChannel(id: string) {
    const next = value.channel_ids.includes(id)
      ? value.channel_ids.filter((c) => c !== id)
      : [...value.channel_ids, id];
    patch({ channel_ids: next });
  }

  function toggleWeekday(day: number) {
    const next = value.weekdays.includes(day)
      ? value.weekdays.filter((d) => d !== day)
      : [...value.weekdays, day].sort((a, b) => a - b);
    patch({ weekdays: next });
  }

  function setEpisodeMode(mode: 'new' | 'all' | 'season') {
    if (mode === 'new') patch({ new_only: true, min_season: null });
    else if (mode === 'all') patch({ new_only: false, min_season: null });
    else patch({ new_only: false, min_season: value.min_season ?? 1 });
  }

  const windowLabel = $derived(value.window_start && value.window_end ? '' : 'any time');
</script>

<section class="editor">
  <div class="identity">
    <div class="poster"></div>
    <div class="identity-text">
      <input
        class="title-input"
        type="text"
        placeholder="Series title"
        value={value.title}
        oninput={(e) => patch({ title: e.currentTarget.value })}
      />
      <input
        class="series-id"
        type="text"
        placeholder="SeriesID"
        value={value.series_id}
        oninput={(e) => patch({ series_id: e.currentTarget.value })}
      />
    </div>
  </div>

  <div class="group">
    <span class="group-label">Channels</span>
    <div class="channel-list">
      {#each channels as channel (channel.id)}
        <label class="channel">
          <input
            type="checkbox"
            checked={value.channel_ids.includes(channel.id)}
            onchange={() => toggleChannel(channel.id)}
          />
          <span class="channel-name" class:on={value.channel_ids.includes(channel.id)}>
            {channel.name}
          </span>
          <span class="lcn">{channel.lcn ? `ch${channel.lcn}` : channel.type_label}</span>
        </label>
      {/each}
      {#if channels.length === 0}
        <span class="hint">No enabled channels — scan a tuner first.</span>
      {/if}
    </div>
    <span class="hint">
      Scoping to a channel list is what separates a first run from a repeats channel — the same
      SeriesID covers both.
    </span>
  </div>

  <div class="group">
    <span class="group-label">Airs</span>
    <div class="chips">
      {#each WEEKDAYS as label, day (label)}
        <button
          type="button"
          class="chip"
          class:on={value.weekdays.includes(day)}
          onclick={() => toggleWeekday(day)}
        >
          {label}
        </button>
      {/each}
    </div>
    <div class="row">
      <input
        class="time-input"
        type="time"
        value={value.window_start ?? ''}
        onchange={(e) => patch({ window_start: e.currentTarget.value || null })}
      />
      <span class="to">to</span>
      <input
        class="time-input"
        type="time"
        value={value.window_end ?? ''}
        onchange={(e) => patch({ window_end: e.currentTarget.value || null })}
      />
      {#if windowLabel}<span class="hint inline">{windowLabel}</span>{/if}
    </div>
  </div>

  <div class="group">
    <span class="group-label">Episodes</span>
    <div class="chips">
      <button
        type="button"
        class="chip"
        class:on={episodeMode === 'new'}
        onclick={() => setEpisodeMode('new')}>New only</button
      >
      <button
        type="button"
        class="chip"
        class:on={episodeMode === 'all'}
        onclick={() => setEpisodeMode('all')}>All</button
      >
      <button
        type="button"
        class="chip"
        class:on={episodeMode === 'season'}
        onclick={() => setEpisodeMode('season')}>From season…</button
      >
      {#if episodeMode === 'season'}
        <input
          class="num-input"
          type="number"
          min="1"
          value={value.min_season ?? 1}
          onchange={(e) => patch({ min_season: Number(e.currentTarget.value) || 1 })}
        />
      {/if}
    </div>
    <span class="hint">
      New = no recording with this episode key. The guide's <code>First</code> flag is not used — it marks
      live sport and news, not first runs.
    </span>
  </div>

  <div class="group">
    <span class="group-label">On a clash</span>
    <div class="chips">
      {#each CLASH as option (option.id)}
        <button
          type="button"
          class="chip"
          class:on={value.on_clash === option.id}
          onclick={() => patch({ on_clash: option.id })}>{option.label}</button
        >
      {/each}
    </div>
    <span class="hint">
      All four tuners busy? Take a repeat inside the guide window instead of losing the episode.
    </span>
  </div>

  <div class="group">
    <span class="group-label">Keep</span>
    <div class="row">
      <input
        class="num-input wide"
        type="number"
        min="0"
        placeholder="all"
        value={value.keep_count ?? ''}
        onchange={(e) =>
          patch({ keep_count: e.currentTarget.value ? Number(e.currentTarget.value) : null })}
      />
      <span class="to">most recent</span>
      <button
        type="button"
        class="chip"
        class:on={value.keep_until === 'watched'}
        onclick={() => patch({ keep_until: value.keep_until === 'watched' ? null : 'watched' })}
        >until watched</button
      >
    </div>
  </div>

  <div class="group">
    <span class="group-label">Padding</span>
    <div class="row">
      <input
        class="num-input"
        type="number"
        min="0"
        value={Math.round(value.padding_before_secs / 60)}
        onchange={(e) => patch({ padding_before_secs: Number(e.currentTarget.value) * 60 })}
      />
      <span class="to">min before</span>
      <input
        class="num-input"
        type="number"
        min="0"
        value={Math.round(value.padding_after_secs / 60)}
        onchange={(e) => patch({ padding_after_secs: Number(e.currentTarget.value) * 60 })}
      />
      <span class="to">min after</span>
    </div>
  </div>
</section>

<style>
  .editor {
    width: 392px;
    flex-shrink: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow-y: auto;
  }

  .identity {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .poster {
    width: 44px;
    height: 60px;
    border-radius: 6px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    flex-shrink: 0;
  }

  .identity-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex-grow: 1;
  }

  .title-input,
  .series-id {
    border: none;
    background: transparent;
    color: var(--ink);
    padding: 0;
    width: 100%;
  }

  .title-input {
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .series-id {
    font-size: 11.5px;
    color: var(--muted);
    font-family: ui-monospace, monospace;
  }

  .title-input:focus,
  .series-id:focus {
    outline: none;
    color: var(--ink);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .group-label {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: #5f6874;
  }

  .channel-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: 168px;
    overflow-y: auto;
  }

  .channel {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 24px;
    cursor: pointer;
  }

  .channel input {
    width: 12px;
    height: 12px;
    accent-color: var(--accent);
    flex-shrink: 0;
    margin: 0;
  }

  .channel-name {
    font-size: 12px;
    color: var(--muted);
    flex-grow: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .channel-name.on {
    color: var(--ink);
  }

  .lcn {
    font-size: 10.5px;
    color: #5f6874;
    flex-shrink: 0;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    align-items: center;
  }

  .chip {
    padding: 5px 11px;
    border-radius: 20px;
    border: 1px solid var(--ring);
    background: transparent;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--muted);
    cursor: pointer;
  }

  .chip.on {
    border-color: var(--accent);
    background: var(--accent-dim);
    color: var(--ink);
  }

  .chip:hover:not(.on) {
    color: var(--ink);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .time-input,
  .num-input {
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    width: 74px;
  }

  .num-input {
    width: 62px;
  }

  .num-input.wide {
    width: 74px;
  }

  .to {
    font-size: 12px;
    color: #5f6874;
  }

  .hint {
    font-size: 10.5px;
    color: #5f6874;
    line-height: 1.45;
  }

  .hint.inline {
    line-height: 1;
  }

  code {
    font-family: ui-monospace, monospace;
  }
</style>
