<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import SeriesRuleList from '$lib/components/livetv/SeriesRuleList.svelte';
  import SeriesRuleEditor from '$lib/components/livetv/SeriesRuleEditor.svelte';
  import SeriesRulePreview from '$lib/components/livetv/SeriesRulePreview.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import * as api from '$lib/api/client';
  import type {
    LivetvChannel,
    LivetvSeriesRule,
    LivetvSeriesRuleInput,
    LivetvSeriesRulePreview
  } from '$lib/types';

  const BLANK: LivetvSeriesRuleInput = {
    series_id: '',
    title: '',
    enabled: true,
    channel_ids: [],
    weekdays: [],
    window_start: null,
    window_end: null,
    new_only: true,
    min_season: null,
    keep_count: null,
    keep_until: null,
    padding_before_secs: 120,
    padding_after_secs: 360,
    on_clash: 'move',
    priority: 0
  };

  let rules = $state<LivetvSeriesRule[]>([]);
  let channels = $state<LivetvChannel[]>([]);
  let selectedId = $state<string | null>(null);
  let creating = $state(false);
  let draft = $state<LivetvSeriesRuleInput>({ ...BLANK });
  let loading = $state(true);
  let error = $state('');
  let saving = $state(false);
  let confirmDelete = $state(false);

  let preview = $state<LivetvSeriesRulePreview | null>(null);
  let previewLoading = $state(false);
  let previewError = $state('');
  let previewTimer: ReturnType<typeof setTimeout> | undefined;
  let previewGeneration = 0;

  const selected = $derived(rules.find((r) => r.id === selectedId) ?? null);
  const dirty = $derived(creating || selected === null ? true : !sameAsSaved(selected, draft));
  const canSave = $derived(
    draft.title.trim().length > 0 &&
      draft.series_id.trim().length > 0 &&
      draft.channel_ids.length > 0
  );

  const headerLine = $derived(
    [
      `${rules.length} rule${rules.length === 1 ? '' : 's'}`,
      `${rules.reduce((n, r) => n + (r.queued_count ?? 0), 0)} recordings queued`
    ].join(' · ')
  );

  function toInput(rule: LivetvSeriesRule): LivetvSeriesRuleInput {
    return {
      series_id: rule.series_id,
      title: rule.title,
      enabled: rule.enabled,
      channel_ids: [...rule.channel_ids],
      weekdays: [...rule.weekdays],
      window_start: rule.window_start,
      window_end: rule.window_end,
      new_only: rule.new_only,
      min_season: rule.min_season,
      keep_count: rule.keep_count,
      keep_until: rule.keep_until,
      padding_before_secs: rule.padding_before_secs,
      padding_after_secs: rule.padding_after_secs,
      on_clash: rule.on_clash,
      priority: rule.priority
    };
  }

  function sameAsSaved(rule: LivetvSeriesRule, input: LivetvSeriesRuleInput): boolean {
    return JSON.stringify(toInput(rule)) === JSON.stringify(input);
  }

  function selectRule(rule: LivetvSeriesRule) {
    creating = false;
    selectedId = rule.id;
    draft = toInput(rule);
    updateUrlParams({ rule: rule.id });
    schedulePreview();
  }

  function startCreate() {
    creating = true;
    selectedId = null;
    draft = { ...BLANK };
    preview = null;
    updateUrlParams({ rule: null });
  }

  function handleChange(next: LivetvSeriesRuleInput) {
    draft = next;
    schedulePreview();
  }

  function schedulePreview() {
    clearTimeout(previewTimer);
    previewTimer = setTimeout(runPreview, 400);
  }

  async function runPreview() {
    if (!canSave) {
      preview = null;
      previewError = '';
      return;
    }
    const gen = ++previewGeneration;
    previewLoading = true;
    try {
      const result = await api.previewLivetvSeriesRule(draft);
      if (gen !== previewGeneration) return;
      preview = result;
      previewError = '';
    } catch (e) {
      if (gen !== previewGeneration) return;
      preview = null;
      previewError =
        'The preview is unavailable — repeat suppression runs on the server against the live guide.';
      console.error('Series rule preview failed:', e);
    } finally {
      if (gen === previewGeneration) previewLoading = false;
    }
  }

  async function load() {
    loading = true;
    try {
      const [ruleList, channelList] = await Promise.all([
        api.getLivetvSeriesRules(),
        api
          .getLivetvChannels({ enabled: true, limit: 500 })
          .then((r) => r.items ?? [])
          .catch(() => [] as LivetvChannel[])
      ]);
      rules = ruleList.items ?? [];
      channels = channelList;
      error = '';

      const wanted = $page.url.searchParams.get('rule');
      const target = rules.find((r) => r.id === wanted) ?? rules[0] ?? null;
      if (target) selectRule(target);
      else startCreate();
    } catch (e) {
      console.error('Failed to load series rules:', e);
      rules = [];
      error = 'Series rules are unavailable. The tuner DVR may not be running yet.';
      startCreate();
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!canSave) return;
    saving = true;
    try {
      const saved = creating
        ? await api.createLivetvSeriesRule(draft)
        : await api.updateLivetvSeriesRule(selectedId!, draft);
      showMessage(creating ? 'Series rule created' : 'Series rule saved');
      const list = await api.getLivetvSeriesRules();
      rules = list.items ?? [];
      creating = false;
      selectedId = saved.id;
      draft = toInput(saved);
      runPreview();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save the rule');
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!selectedId) return;
    try {
      await api.deleteLivetvSeriesRule(selectedId);
      showMessage('Series rule deleted');
      confirmDelete = false;
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete the rule');
    }
  }

  async function togglePaused() {
    if (!selectedId) return;
    const next = !draft.enabled;
    draft = { ...draft, enabled: next };
    try {
      const saved = await api.updateLivetvSeriesRule(selectedId, { enabled: next });
      rules = rules.map((r) => (r.id === saved.id ? saved : r));
    } catch (e) {
      draft = { ...draft, enabled: !next };
      showMessage(e instanceof Error ? e.message : 'Failed to update the rule');
    }
  }

  onMount(load);
  onDestroy(() => clearTimeout(previewTimer));
</script>

<svelte:head><title>Live TV series rules · kobbler</title></svelte:head>

<div class="page-container rules-page">
  <header class="head">
    <div class="titles">
      <h1>Series rules</h1>
      <span class="subtitle">{headerLine}</span>
    </div>
    <div class="head-actions">
      <a class="link-chip" href="/livetv/recordings">Recordings</a>
      {#if !creating && selectedId}
        <button type="button" class="btn-sec" onclick={togglePaused}>
          {draft.enabled ? 'Pause rule' : 'Resume rule'}
        </button>
        <button type="button" class="btn-danger" onclick={() => (confirmDelete = true)}>
          Delete
        </button>
      {/if}
      <button
        type="button"
        class="btn-primary"
        disabled={!canSave || saving || !dirty}
        onclick={save}
      >
        {saving ? 'Saving…' : creating ? 'Create rule' : 'Save rule'}
      </button>
    </div>
  </header>

  {#if error}
    <ErrorBanner message={error} onRetry={load} />
  {/if}

  <div class="layout">
    <SeriesRuleList {rules} {selectedId} {creating} onSelect={selectRule} onCreate={startCreate} />
    <SeriesRuleEditor value={draft} {channels} onChange={handleChange} />
    <SeriesRulePreview {preview} loading={previewLoading} error={previewError} />
  </div>

  {#if loading}
    <div class="loading-card">Loading rules…</div>
  {/if}
</div>

{#if confirmDelete && selected}
  <DeleteConfirmModal
    title="Delete series rule?"
    message={`Delete the rule for ${selected.title}? Queued recordings it created are cancelled.`}
    onConfirm={remove}
    onCancel={() => (confirmDelete = false)}
  />
{/if}

<style>
  .rules-page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 48px);
    box-sizing: border-box;
  }

  .head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    margin-bottom: 16px;
    flex-shrink: 0;
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

  .layout {
    display: flex;
    gap: 16px;
    flex-grow: 1;
    min-height: 0;
  }

  @media (max-width: 1200px) {
    .rules-page {
      height: auto;
    }

    .layout {
      flex-wrap: wrap;
    }
  }
</style>
