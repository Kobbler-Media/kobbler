<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    onReconnect,
    offReconnect,
    on as onLiveEvent,
    off as offLiveEvent
  } from '#lib/services/eventstream.svelte.js';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import LivetvTabs from '#lib/components/livetv/LivetvTabs.svelte';
  import ScheduledRow from '#lib/components/livetv/ScheduledRow.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import * as api from '#lib/api/client.js';
  import type { LivetvRecording, LivetvSeriesRule } from '#lib/types.js';

  let queued = $state<LivetvRecording[]>([]);
  let rules = $state<LivetvSeriesRule[]>([]);
  let loading = $state(true);
  let error = $state('');
  let confirmCancel = $state<LivetvRecording | null>(null);
  let busyId = $state<string | null>(null);

  let loadGeneration = 0;
  let reconnectHandler: (() => void) | null = null;
  let recordingHandler: ((data: unknown) => void) | null = null;

  const days = $derived.by(() => {
    const groups = new Map<string, LivetvRecording[]>();
    for (const rec of [...queued].sort(
      (a, b) => Date.parse(a.scheduled_start) - Date.parse(b.scheduled_start)
    )) {
      const d = new Date(rec.scheduled_start);
      const key = isNaN(d.getTime())
        ? 'Unscheduled'
        : d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'short' });
      const list = groups.get(key);
      if (list) list.push(rec);
      else groups.set(key, [rec]);
    }
    return [...groups.entries()].map(([label, items]) => ({ label, items }));
  });

  const ruleQueueCount = $derived(rules.reduce((n, r) => n + (r.queued_count ?? 0), 0));

  const headerLine = $derived([
    `${queued.length} queued`,
    rules.length > 0 ? `${rules.length} series rule${rules.length === 1 ? '' : 's'}` : null,
    ruleQueueCount > 0 ? `${ruleQueueCount} from rules` : null
  ]);

  async function load() {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const [recordings, ruleList] = await Promise.all([
        api.getLivetvRecordings({ status: 'scheduled', limit: 200 }),
        api.getLivetvSeriesRules().catch(() => ({ items: [] as LivetvSeriesRule[] }))
      ]);
      if (gen !== loadGeneration) return;
      queued = recordings.items ?? [];
      rules = ruleList.items ?? [];
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      console.error('Failed to load scheduled recordings:', e);
      queued = [];
      error = 'The schedule is unavailable. The tuner DVR may not be running yet.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function cancel(recording: LivetvRecording) {
    busyId = recording.id;
    try {
      await api.deleteLivetvRecording(recording.id);
      showMessage('Recording cancelled');
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to cancel the recording');
    } finally {
      busyId = null;
      confirmCancel = null;
    }
  }

  onMount(() => {
    load();
    reconnectHandler = () => load();
    onReconnect(reconnectHandler);
    recordingHandler = () => load();
    onLiveEvent('livetv_recording_updated', recordingHandler);
    onLiveEvent('livetv_guide_synced', recordingHandler);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (recordingHandler) {
      offLiveEvent('livetv_recording_updated', recordingHandler);
      offLiveEvent('livetv_guide_synced', recordingHandler);
    }
  });
</script>

<svelte:head><title>Live TV scheduled · kobbler</title></svelte:head>

<div class="page-container">
  <header class="head">
    <div class="titles">
      <h1>Scheduled</h1>
      <span class="subtitle"><MetaLine items={headerLine} /></span>
    </div>
    <a class="link-chip" href="/livetv/series-rules">Series rules</a>
  </header>

  <LivetvTabs active="scheduled" />

  {#if error}
    <ErrorBanner message={error} onRetry={load} />
  {/if}

  {#if loading && queued.length === 0}
    <div class="loading-card">Loading the schedule…</div>
  {:else if queued.length === 0}
    <div class="empty-state">
      <h3>Nothing queued</h3>
      <p class="muted">
        Rules re-run on every guide refresh rather than scheduling a week out — the guide only
        reaches 48 hours, so a queue this short is normal.
      </p>
    </div>
  {:else}
    {#each days as day (day.label)}
      <section class="day">
        <h2>{day.label}</h2>
        <div class="rows">
          {#each day.items as recording (recording.id)}
            <ScheduledRow
              {recording}
              rule={rules.find((r) => r.id === recording.series_rule_id) ?? null}
              busy={busyId === recording.id}
              onCancel={(r) => (confirmCancel = r)}
            />
          {/each}
        </div>
      </section>
    {/each}
  {/if}
</div>

{#if confirmCancel}
  <DeleteConfirmModal
    title="Cancel recording?"
    message={`Drop the queued recording of ${confirmCancel.title}?`}
    confirmLabel="Cancel recording"
    cancelLabel="Keep it"
    onConfirm={() => confirmCancel && cancel(confirmCancel)}
    onCancel={() => (confirmCancel = null)}
  />
{/if}

<style>
  .head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    margin-bottom: 14px;
  }

  .titles {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  .subtitle {
    font-size: 12px;
    color: var(--muted);
  }

  .link-chip {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    height: 34px;
    padding: 0 14px;
    border-radius: 9px;
    border: 1px solid var(--ring);
    font-size: 12.5px;
    font-weight: 600;
    color: var(--muted);
    text-decoration: none;
  }

  .link-chip:hover {
    color: var(--ink);
    border-color: var(--muted);
  }

  .day {
    margin-top: 18px;
  }

  h2 {
    margin: 0 0 9px;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
    letter-spacing: 0.02em;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
</style>
