<script lang="ts">
  import { onMount } from 'svelte';
  import { getJobConcurrency, updateJobConcurrency } from '#lib/api/client.js';
  import type { JobConcurrencyItem } from '#lib/api/client.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';

  let concurrencyJobs = $state<JobConcurrencyItem[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let savingJob = $state<string | null>(null);
  let pendingEdits = $state<Record<string, number>>({});

  const resourceGroups = $derived(() => {
    const groups = new Map<string, JobConcurrencyItem[]>();
    for (const job of concurrencyJobs) {
      const group = job.resource_group;
      if (!groups.has(group)) groups.set(group, []);
      groups.get(group)!.push(job);
    }
    return [...groups.entries()];
  });

  onMount(async () => {
    await loadConcurrency();
  });

  const loadConcurrency = async () => {
    loading = true;
    error = null;
    try {
      const resp = await getJobConcurrency();
      concurrencyJobs = resp.jobs;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load concurrency settings';
    } finally {
      loading = false;
    }
  };

  const handleConcurrencyChange = (jobType: string, value: string) => {
    const num = parseInt(value, 10);
    if (num >= 1 && num <= 32) {
      pendingEdits = { ...pendingEdits, [jobType]: num };
    }
  };

  const saveConcurrency = async (jobType: string) => {
    const val = pendingEdits[jobType];
    if (val == null) return;
    savingJob = jobType;
    try {
      await updateJobConcurrency(jobType, val);
      const { [jobType]: _, ...rest } = pendingEdits;
      pendingEdits = rest;
      await loadConcurrency();
      showMessage('Concurrency updated — takes effect on restart');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save');
    } finally {
      savingJob = null;
    }
  };

  const resetConcurrency = async (jobType: string) => {
    const job = concurrencyJobs.find((j) => j.job_type === jobType);
    if (!job || job.is_default) return;
    savingJob = jobType;
    try {
      await updateJobConcurrency(jobType, job.default);
      const { [jobType]: _, ...rest } = pendingEdits;
      pendingEdits = rest;
      await loadConcurrency();
      showMessage('Reset to default — takes effect on restart');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to reset');
    } finally {
      savingJob = null;
    }
  };
</script>

<svelte:head>
  <title>Job Settings — Kobbler</title>
</svelte:head>

<div class="concurrency-section" data-field="job_concurrency">
  <div class="section-header">
    <h2>Job Concurrency</h2>
    <span class="restart-note">Changes take effect on restart</span>
  </div>

  {#if loading}
    <div class="loading-card">Loading…</div>
  {:else if error}
    <div class="error">{error}</div>
  {:else}
    {#each resourceGroups() as [group, jobs] (group)}
      <div class="concurrency-group">
        <h3 class="group-label">{group}</h3>
        <div class="concurrency-list">
          {#each jobs as job (job.job_type)}
            {@const edited = pendingEdits[job.job_type]}
            {@const currentVal = edited ?? job.concurrency}
            {@const isDirty = edited != null && edited !== job.concurrency}
            <div class="concurrency-row" class:modified={isDirty}>
              <div class="concurrency-info">
                <div class="concurrency-name">{job.name}</div>
                <div class="concurrency-desc muted">{job.description}</div>
                <div class="concurrency-meta muted">
                  <MetaLine>
                    {#if job.resource_notes}<span>{job.resource_notes}</span>{/if}
                    {#if job.pending > 0 || job.running > 0}
                      <span class="queue-counts">
                        {job.running} running, {job.pending} pending
                      </span>
                    {/if}
                  </MetaLine>
                </div>
              </div>
              <div class="concurrency-controls">
                <input
                  type="number"
                  class="concurrency-input"
                  value={currentVal}
                  oninput={(e) =>
                    handleConcurrencyChange(job.job_type, (e.target as HTMLInputElement).value)}
                  min="1"
                  max="32"
                  disabled={savingJob === job.job_type}
                />
                {#if !job.is_default && !isDirty}
                  <button
                    class="btn-reset"
                    type="button"
                    title="Reset to default ({job.default})"
                    onclick={() => resetConcurrency(job.job_type)}
                    disabled={savingJob === job.job_type}
                  >
                    ↺
                  </button>
                {/if}
                {#if isDirty}
                  <button
                    class="btn-save"
                    type="button"
                    onclick={() => saveConcurrency(job.job_type)}
                    disabled={savingJob === job.job_type}
                  >
                    {savingJob === job.job_type ? '…' : 'Save'}
                  </button>
                {/if}
                {#if !job.is_default}
                  <span class="default-badge" title="Default: {job.default}">
                    d:{job.default}
                  </span>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/each}
  {/if}
</div>

<style>
  .concurrency-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .restart-note {
    font-size: 11px;
    color: var(--muted);
    padding: 3px 8px;
    background: var(--ring);
    border-radius: 6px;
  }

  .concurrency-group {
    display: grid;
    gap: 6px;
  }

  .group-label {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
    margin: 0;
    padding: 4px 0;
  }

  .concurrency-list {
    display: grid;
    gap: 6px;
  }

  .concurrency-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 10px 16px;
    transition: border-color 0.15s;
  }

  .concurrency-row.modified {
    border-color: var(--accent);
  }

  .concurrency-info {
    min-width: 0;
    flex: 1;
  }

  .concurrency-name {
    font-weight: 600;
    font-size: 13px;
  }

  .concurrency-desc {
    font-size: 12px;
    margin-top: 2px;
  }

  .concurrency-meta {
    font-size: 11px;
    margin-top: 2px;
  }

  .queue-counts {
    color: var(--accent);
    font-weight: 500;
  }

  .concurrency-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .concurrency-input {
    width: 56px;
    padding: 4px 6px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 13px;
    font-weight: 600;
    text-align: center;
    font-family: inherit;
  }

  .concurrency-input:focus {
    border-color: var(--accent);
    outline: none;
  }

  .btn-save {
    padding: 4px 10px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
  }

  .btn-save:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-reset {
    padding: 4px 6px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--muted);
    font-size: 14px;
    cursor: pointer;
    line-height: 1;
  }

  .btn-reset:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--ink);
  }

  .btn-reset:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .default-badge {
    font-size: 10px;
    color: var(--muted);
    padding: 2px 5px;
    background: var(--ring);
    border-radius: 4px;
    white-space: nowrap;
  }

  @media (max-width: 768px) {
    .concurrency-row {
      flex-direction: column;
      align-items: stretch;
      gap: 8px;
    }

    .concurrency-controls {
      justify-content: flex-end;
    }
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }
</style>
