<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import {
    onReconnect,
    offReconnect,
    on as onLiveEvent,
    off as offLiveEvent
  } from '$lib/services/eventstream.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import OnNowNotice from '$lib/components/livetv/OnNowNotice.svelte';
  import OnNowCard from '$lib/components/livetv/OnNowCard.svelte';
  import ConfidenceDots from '$lib/components/livetv/ConfidenceDots.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { openLivetvChannel } from '$lib/stores/livetvPlayer.svelte';
  import * as api from '$lib/api/client';
  import type { LivetvOnNow, LivetvOnNowItem } from '$lib/types';

  let data = $state<LivetvOnNow | null>(null);
  let loading = $state(true);
  let error = $state('');
  let noticeDismissed = $state(false);
  let noticeBusy = $state(false);
  const busyGuides = new SvelteSet<string>();

  let loadGeneration = 0;
  let reconnectHandler: (() => void) | null = null;
  let guideHandler: ((payload: unknown) => void) | null = null;

  const clock = new Intl.DateTimeFormat(undefined, {
    weekday: 'long',
    hour: '2-digit',
    minute: '2-digit'
  });

  const generatedLabel = $derived.by(() => {
    if (!data) return 'ranked from your own watch history, nothing leaves the house';
    const d = new Date(data.generated_at);
    const when = isNaN(d.getTime()) ? '' : `${clock.format(d)} · `;
    return `${when}ranked from your own watch history, nothing leaves the house`;
  });

  const groups = $derived(data?.groups.filter((g) => g.items.length > 0) ?? []);

  async function load() {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const result = await api.getLivetvOnNow();
      if (gen !== loadGeneration) return;
      data = result;
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      console.error('Failed to load On now:', e);
      data = null;
      error =
        'On now is unavailable. Recommendations are built from local watch history, which the tuner writes as you watch.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  function watch(item: LivetvOnNowItem) {
    openLivetvChannel(item.channel);
  }

  async function record(item: LivetvOnNowItem) {
    busyGuides.add(item.guide.id);
    try {
      await api.createLivetvRecording({ guide_id: item.guide.id });
      showMessage(`Recording ${item.guide.title}`);
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to schedule the recording');
    } finally {
      busyGuides.delete(item.guide.id);
    }
  }

  async function recordAlternative(guideId: string) {
    noticeBusy = true;
    try {
      await api.createLivetvRecording({ guide_id: guideId });
      showMessage('Recording the later airing instead');
      noticeDismissed = true;
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to schedule the recording');
    } finally {
      noticeBusy = false;
    }
  }

  onMount(() => {
    load();
    reconnectHandler = () => load();
    onReconnect(reconnectHandler);
    guideHandler = () => load();
    onLiveEvent('livetv_guide_synced', guideHandler);
    onLiveEvent('livetv_recording_updated', guideHandler);
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (guideHandler) {
      offLiveEvent('livetv_guide_synced', guideHandler);
      offLiveEvent('livetv_recording_updated', guideHandler);
    }
  });
</script>

<svelte:head><title>On now · kobbler</title></svelte:head>

<div class="page-container">
  <header class="head">
    <div class="titles">
      <h1>On now</h1>
      <span class="subtitle">{generatedLabel}</span>
    </div>
    <div class="head-actions">
      <a class="link-chip" href="/livetv">Full guide</a>
      <a class="link-chip" href="/livetv/channels">All channels</a>
    </div>
  </header>

  {#if error}
    <ErrorBanner message={error} onRetry={load} />
  {/if}

  {#if data?.tuner_notice && !noticeDismissed}
    <OnNowNotice
      notice={data.tuner_notice}
      busy={noticeBusy}
      onIgnore={() => (noticeDismissed = true)}
      onRecordAlternative={recordAlternative}
    />
  {/if}

  {#if loading && !data}
    <div class="loading-card">Ranking what is on…</div>
  {:else if groups.length === 0}
    <div class="empty-state">
      <h3>Nothing to suggest yet</h3>
      <p class="muted">
        Ranking needs watch history — count, how much of each programme you finished, and how
        regular the slot is. Watch a few things and this fills in.
      </p>
    </div>
  {:else}
    <div class="groups">
      {#each groups as group (group.key + group.title)}
        <section class="group">
          <div class="group-head">
            <span class="group-title">{group.title}</span>
            {#if group.subtitle}<span class="group-sub">{group.subtitle}</span>{/if}
          </div>
          <div class="rail">
            {#each group.items as item (item.guide.id)}
              <OnNowCard
                {item}
                compact={group.key !== 'series'}
                busy={busyGuides.has(item.guide.id)}
                onWatch={watch}
                onRecord={record}
              />
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {/if}

  <footer class="explainer">
    <ConfidenceDots filled={2} />
    <span>
      Confidence comes from watch count, how much of each programme you finished, and how regular
      the slot is — a show you start and bail on scores lower than one you never miss.
    </span>
  </footer>
</div>

<style>
  .head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    margin-bottom: 16px;
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

  .head-actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .link-chip {
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

  .groups {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .group-head {
    display: flex;
    align-items: baseline;
    gap: 11px;
  }

  .group-title {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .group-sub {
    font-size: 11.5px;
    color: #5f6874;
  }

  .rail {
    display: flex;
    gap: 12px;
    overflow-x: auto;
    padding-bottom: 4px;
  }

  .explainer {
    display: flex;
    align-items: center;
    gap: 9px;
    padding-top: 14px;
    margin-top: 20px;
    border-top: 1px solid var(--ring);
    font-size: 11.5px;
    color: var(--muted);
    line-height: 1.5;
  }
</style>
