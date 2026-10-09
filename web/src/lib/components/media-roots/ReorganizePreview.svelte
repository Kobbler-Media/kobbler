<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Modal } from '@koblibs/ui';
  import { triggerMediaRootReorganize, getJob } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { on, off } from '$lib/services/eventstream.svelte';
  import type { Job, JobEvent } from '$lib/types';

  interface Props {
    rootId: string;
    rootName: string;
    rootPath: string;
    open: boolean;
    onClose: () => void;
  }

  let { rootId, rootName, rootPath, open, onClose }: Props = $props();

  let loading = $state(false);
  let mode = $state<'idle' | 'dry_run' | 'execute'>('idle');
  let jobResult = $state<Job['result']>(null);
  let jobFailed = $state(false);
  let jobRunning = $state(false);
  let includeUnconfirmed = $state(false);

  type DryRunChange = { from: string; to: string; unconfirmed?: boolean };

  const renamedCount = $derived((jobResult?.renamed as number | undefined) ?? 0);
  const renamedUnconfirmedCount = $derived(
    (jobResult?.renamed_unconfirmed as number | undefined) ?? 0
  );
  const alreadyCorrectCount = $derived((jobResult?.already_correct as number | undefined) ?? 0);
  const errorCount = $derived((jobResult?.errors as number | undefined) ?? 0);
  const dryRunChanges = $derived((jobResult?.changes as DryRunChange[] | undefined) ?? []);
  const dryRunTruncated = $derived((jobResult?.truncated as boolean | undefined) ?? false);
  const totalRenamed = $derived(renamedCount + renamedUnconfirmedCount);

  let jobCallback: ((data: unknown) => void) | null = null;

  function handleJobResult(job: Job) {
    jobResult = job.result;
    jobFailed = job.status === 'failed';
    if (job.status === 'failed') {
      showMessage(`Job failed: ${job.error ?? 'Unknown error'}`);
    }
    jobRunning = false;
  }

  function subscribeToJob(id: string) {
    unsubscribeFromJob();
    jobCallback = on('job', async (data: unknown) => {
      const event = data as JobEvent;
      if (event.job_id !== id) return;
      if (event.status === 'completed' || event.status === 'failed') {
        try {
          handleJobResult(await getJob(id));
        } catch {
          showMessage('Failed to fetch job result');
        }
      }
    });

    // Catch-up: if the job completed before we registered the SSE callback,
    // resolve immediately instead of spinning forever.
    getJob(id)
      .then((job) => {
        if (!jobRunning) return;
        if (job.status === 'completed' || job.status === 'failed') {
          handleJobResult(job);
        }
      })
      .catch(() => {});
  }

  function unsubscribeFromJob() {
    if (jobCallback) {
      off('job', jobCallback);
      jobCallback = null;
    }
  }

  onDestroy(unsubscribeFromJob);

  async function runDryRun() {
    loading = true;
    mode = 'dry_run';
    jobResult = null;
    jobFailed = false;
    jobRunning = false;
    try {
      const result = await triggerMediaRootReorganize(rootId, true, includeUnconfirmed);
      jobRunning = true;
      subscribeToJob(result.job_id);
    } catch (e) {
      showMessage(`Dry-run failed: ${e instanceof Error ? e.message : 'Unknown error'}`);
      mode = 'idle';
    } finally {
      loading = false;
    }
  }

  async function runReorganize() {
    loading = true;
    mode = 'execute';
    jobResult = null;
    jobFailed = false;
    jobRunning = false;
    try {
      const result = await triggerMediaRootReorganize(rootId, false, includeUnconfirmed);
      jobRunning = true;
      subscribeToJob(result.job_id);
    } catch (e) {
      showMessage(`Reorganize failed: ${e instanceof Error ? e.message : 'Unknown error'}`);
      mode = 'idle';
    } finally {
      loading = false;
    }
  }

  function stripRootPath(filePath: string): string {
    if (filePath.startsWith(rootPath)) {
      const rel = filePath.slice(rootPath.length);
      return rel.startsWith('/') ? rel.slice(1) : rel;
    }
    return filePath;
  }

  function handleClose() {
    unsubscribeFromJob();
    jobResult = null;
    jobFailed = false;
    jobRunning = false;
    mode = 'idle';
    loading = false;
    onClose();
  }
</script>

{#if open}
  <Modal title="Reorganize: {rootName}" width="min(90vw, 600px)" onClose={handleClose}>
    <div class="body">
      <p class="description">
        Reorganize renames and moves files in this media root to match the configured naming
        template.
      </p>

      <label class="toggle-row">
        <input type="checkbox" bind:checked={includeUnconfirmed} disabled={loading || jobRunning} />
        <span>Include unconfirmed files (review/ambiguous)</span>
      </label>

      {#if loading}
        <div class="status-box">
          <span class="spinner"></span>
          <span>{mode === 'dry_run' ? 'Starting dry-run...' : 'Starting reorganize...'}</span>
        </div>
      {:else if jobRunning}
        <div class="status-box">
          <span class="spinner"></span>
          <span>{mode === 'dry_run' ? 'Running dry-run...' : 'Reorganizing files...'}</span>
        </div>
      {:else if jobResult && mode === 'dry_run'}
        <div class="results">
          <div class="counts">
            <div class="count-item">
              <span class="count-value">{renamedCount}</span>
              <span class="count-label">would rename</span>
            </div>
            {#if renamedUnconfirmedCount > 0}
              <div class="count-item unconfirmed">
                <span class="count-value">{renamedUnconfirmedCount}</span>
                <span class="count-label">unconfirmed</span>
              </div>
            {/if}
            <div class="count-item">
              <span class="count-value">{alreadyCorrectCount}</span>
              <span class="count-label">already correct</span>
            </div>
            <div class="count-item">
              <span class="count-value">{errorCount}</span>
              <span class="count-label">errors</span>
            </div>
          </div>

          {#if dryRunChanges.length > 0}
            <div class="changes-list">
              {#each dryRunChanges as change (change.from)}
                <div class="change-row" class:unconfirmed={change.unconfirmed}>
                  <div class="change-from">{stripRootPath(change.from)}</div>
                  <span class="change-arrow material-icons-outlined">arrow_forward</span>
                  <div class="change-to">{stripRootPath(change.to)}</div>
                </div>
              {/each}
            </div>
            {#if dryRunTruncated}
              <p class="truncated-note">
                Showing first {dryRunChanges.length} of {totalRenamed} changes
              </p>
            {/if}
          {:else if totalRenamed === 0}
            <p class="hint">All files already match the naming template.</p>
          {/if}
        </div>
      {:else if jobResult && mode === 'execute'}
        <div class="status-box success">
          <span class="label">Reorganize complete</span>
          <p class="hint">
            Renamed {renamedCount} files{renamedUnconfirmedCount > 0
              ? ` (+${renamedUnconfirmedCount} unconfirmed)`
              : ''}, {alreadyCorrectCount} already correct{errorCount > 0
              ? `, ${errorCount} errors`
              : ''}.
          </p>
        </div>
      {:else if jobFailed}
        <div class="status-box error">
          <span class="label">Job failed</span>
          <p class="hint">Check the <a href="/jobs">Jobs page</a> for details.</p>
        </div>
      {:else}
        <p class="hint">
          Use <strong>Preview</strong> to see what would change without modifying any files. Use
          <strong>Reorganize Now</strong> to apply changes immediately.
        </p>
      {/if}
    </div>

    {#snippet footer()}
      <button class="btn alt" type="button" onclick={handleClose}>
        {jobResult ? 'Close' : 'Cancel'}
      </button>
      <button class="btn alt" type="button" onclick={runDryRun} disabled={loading || jobRunning}>
        {loading && mode === 'dry_run' ? 'Starting...' : 'Preview (Dry Run)'}
      </button>
      <button
        class="btn danger"
        type="button"
        onclick={runReorganize}
        disabled={loading || jobRunning}
      >
        {loading && mode === 'execute' ? 'Starting...' : 'Reorganize Now'}
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .description {
    margin: 0;
    font-size: 13px;
    color: var(--ink);
    line-height: 1.5;
  }

  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
    line-height: 1.5;
  }

  .hint a {
    color: var(--accent);
    text-decoration: underline;
  }

  .toggle-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--ink);
    cursor: pointer;
  }

  .toggle-row input[type='checkbox'] {
    accent-color: var(--accent);
  }

  .status-box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--bg, transparent);
    font-size: 13px;
    text-align: center;
  }

  .status-box.success {
    border-color: var(--accent);
  }

  .status-box.error {
    border-color: #dc3545;
  }

  .label {
    font-weight: 600;
    font-size: 13px;
  }

  .results {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .counts {
    display: flex;
    gap: 16px;
    justify-content: center;
  }

  .count-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .count-item.unconfirmed .count-value {
    color: var(--accent);
  }

  .count-value {
    font-size: 20px;
    font-weight: 700;
    color: var(--ink);
  }

  .count-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.3px;
  }

  .changes-list {
    max-height: 300px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 8px;
  }

  .change-row {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    gap: 6px;
    align-items: center;
    padding: 4px 6px;
    border-radius: 4px;
    font-size: 11px;
    font-family: monospace;
  }

  .change-row:nth-child(odd) {
    background: var(--bg);
  }

  .change-row.unconfirmed {
    border-left: 2px solid var(--accent);
  }

  .change-from {
    color: var(--muted);
    word-break: break-all;
    text-align: right;
  }

  .change-arrow {
    font-size: 14px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .change-to {
    color: var(--ink);
    word-break: break-all;
  }

  .truncated-note {
    margin: 0;
    font-size: 11px;
    color: var(--muted);
    text-align: center;
    font-style: italic;
  }

  .spinner {
    width: 20px;
    height: 20px;
    border: 2px solid var(--ring);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .btn {
    border-radius: 999px;
    padding: 8px 14px;
    border: 1px solid #3b2b1f;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    letter-spacing: 0.6px;
    text-transform: uppercase;
    cursor: pointer;
    transition:
      transform 0.12s ease,
      box-shadow 0.12s ease;
  }

  .btn:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  .btn:not(:disabled):hover {
    transform: translateY(-1px);
    box-shadow: 0 6px 14px rgba(0, 0, 0, 0.2);
  }

  .btn.alt {
    background: var(--accent-2);
    color: #e7f4f2;
    border-color: #1d3a38;
  }

  .btn.danger {
    background: #dc3545;
    color: white;
    border-color: #a71d2a;
  }

  .btn.danger:not(:disabled):hover {
    background: #c82333;
  }
</style>
