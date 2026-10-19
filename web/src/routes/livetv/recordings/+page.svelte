<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { formatSize } from '$lib/utils/format';
  import {
    onReconnect,
    offReconnect,
    on as onLiveEvent,
    off as offLiveEvent
  } from '$lib/services/eventstream.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import LivetvTabs from '$lib/components/livetv/LivetvTabs.svelte';
  import TunerTimeline from '$lib/components/livetv/TunerTimeline.svelte';
  import RecordingRow from '$lib/components/livetv/RecordingRow.svelte';
  import RecordingPlayer from '$lib/components/livetv/player/RecordingPlayer.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import { getLivetvState, fetchLivetvTuners, fetchLivetvLeases } from '$lib/stores/livetv.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import * as api from '$lib/api/client';
  import type { LivetvRecording } from '$lib/types';

  const livetv = getLivetvState();

  let recordings = $state<LivetvRecording[]>([]);
  let loading = $state(true);
  let error = $state('');
  let playing = $state<LivetvRecording | null>(null);
  let confirmRemove = $state<LivetvRecording | null>(null);
  let busyId = $state<string | null>(null);

  let loadGeneration = 0;
  let reconnectHandler: (() => void) | null = null;
  let recordingHandler: ((data: unknown) => void) | null = null;

  const inProgress = $derived(recordings.filter((r) => r.status === 'recording'));
  const queued = $derived(recordings.filter((r) => r.status === 'scheduled'));
  const finished = $derived(
    recordings.filter((r) => r.status !== 'recording' && r.status !== 'scheduled')
  );

  const bytesUsed = $derived(recordings.reduce((n, r) => n + (r.size_bytes ?? 0), 0));

  const headerLine = $derived([
    `${inProgress.length} recording now`,
    `${queued.length} queued`,
    bytesUsed > 0 ? `${formatSize(bytesUsed)} used` : null
  ]);

  async function load() {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const result = await api.getLivetvRecordings({ limit: 200 });
      if (gen !== loadGeneration) return;
      recordings = result.items ?? [];
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      console.error('Failed to load Live TV recordings:', e);
      recordings = [];
      error = 'Recordings are unavailable. The tuner DVR may not be running yet.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function reload() {
    await Promise.all([load(), fetchLivetvLeases()]);
  }

  async function removeRecording(recording: LivetvRecording) {
    busyId = recording.id;
    try {
      if (recording.status === 'recording') {
        await api.stopLivetvRecording(recording.id);
        showMessage('Recording stopped');
      } else {
        await api.deleteLivetvRecording(recording.id);
        showMessage('Recording removed');
      }
      await reload();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update the recording');
    } finally {
      busyId = null;
      confirmRemove = null;
    }
  }

  onMount(() => {
    fetchLivetvTuners();
    reload();

    reconnectHandler = () => reload();
    onReconnect(reconnectHandler);
    recordingHandler = () => reload();
    onLiveEvent('livetv_recording_updated', recordingHandler);
    onLiveEvent('livetv_lease_changed', recordingHandler);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (recordingHandler) {
      offLiveEvent('livetv_recording_updated', recordingHandler);
      offLiveEvent('livetv_lease_changed', recordingHandler);
    }
  });
</script>

<svelte:head><title>Live TV recordings · kobbler</title></svelte:head>

<div class="page-container">
  <header class="head">
    <div class="titles">
      <h1>Recordings</h1>
      <span class="subtitle"><MetaLine items={headerLine} /></span>
    </div>
    <a class="link-chip" href="/livetv/series-rules">Series rules</a>
  </header>

  <LivetvTabs active="recordings" />

  {#if error}
    <ErrorBanner message={error} onRetry={reload} />
  {/if}

  <TunerTimeline tuners={livetv.tuners} leases={livetv.leases} {recordings} />

  <div class="rows">
    {#if loading && recordings.length === 0}
      <div class="loading-card">Loading recordings…</div>
    {:else if recordings.length === 0}
      <div class="empty-state">
        <h3>Nothing recorded yet</h3>
        <p class="muted">
          Record from the guide, or hit record while watching — the buffer is promoted from the
          moment you tuned in, so the recording starts where you did.
        </p>
      </div>
    {:else}
      {#each [...inProgress, ...finished] as recording (recording.id)}
        <RecordingRow
          {recording}
          busy={busyId === recording.id}
          onPlay={(r) => (playing = r)}
          onCancel={(r) => (confirmRemove = r)}
        />
      {/each}
    {/if}
  </div>
</div>

{#if playing}
  <RecordingPlayer
    recording={playing}
    onClose={() => {
      playing = null;
      reload();
    }}
  />
{/if}

{#if confirmRemove}
  <DeleteConfirmModal
    title={confirmRemove.status === 'recording' ? 'Stop recording?' : 'Delete recording?'}
    message={confirmRemove.status === 'recording'
      ? `Stop recording ${confirmRemove.title}? What is already on disk is kept.`
      : `Delete ${confirmRemove.title} and its files?`}
    confirmLabel={confirmRemove.status === 'recording' ? 'Stop' : 'Delete'}
    onConfirm={() => confirmRemove && removeRecording(confirmRemove)}
    onCancel={() => (confirmRemove = null)}
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
    gap: 7px;
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

  .rows {
    display: flex;
    flex-direction: column;
    gap: 9px;
    margin-top: 16px;
  }
</style>
