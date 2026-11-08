<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { SearchSelect } from '@koblibs/ui';
  import { updateUrlParams } from '#lib/utils/url-state.js';
  import { getJobs, deleteJob, cancelJob, deleteCompletedJobs } from '#lib/api/client.js';
  import { groupJobs } from '#lib/utils/job-groups.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import { formatJobType, onJobChange } from '#lib/stores/jobs.svelte.js';
  import { entityRoute, entityTypeLabel } from '#lib/utils/entity-routes.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import type { Job } from '#lib/types.js';

  const jobsPageSize = 20;

  let jobsList = $state<Job[]>([]);
  let jobsTotal = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let expandedJobId = $state<string | null>(null);
  let expandedGroups = $state<Set<string>>(new Set());
  let reconnectHandler: (() => void) | null = null;
  let jobChangeUnsub: (() => void) | null = null;

  const groupedJobs = $derived(groupJobs(jobsList));

  const urlJobsPage = $derived(parseInt(page.url.searchParams.get('page') || '0', 10));
  const urlJobsStatus = $derived(page.url.searchParams.get('status') || '');
  const urlShowRoutine = $derived(page.url.searchParams.get('routine') === '1');

  const totalJobPages = $derived(Math.ceil(jobsTotal / jobsPageSize));

  const jobStatusOptions = [
    { value: '', label: 'All statuses' },
    { value: 'pending', label: 'Pending' },
    { value: 'running', label: 'Running' },
    { value: 'completed', label: 'Completed' },
    { value: 'failed', label: 'Failed' }
  ];

  onMount(async () => {
    reconnectHandler = onReconnect(() => {
      loadJobsList();
    });
    jobChangeUnsub = onJobChange(() => {
      loadJobsList();
    });

    await loadJobsList();
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (jobChangeUnsub) jobChangeUnsub();
  });

  $effect(() => {
    const _page = urlJobsPage;
    const _status = urlJobsStatus;
    const _routine = urlShowRoutine;
    loadJobsList();
  });

  const loadJobsList = async () => {
    loading = true;
    error = null;
    try {
      const response = await getJobs({
        limit: jobsPageSize,
        offset: urlJobsPage * jobsPageSize,
        status: urlJobsStatus || undefined,
        exclude_routine: !urlShowRoutine
      });
      jobsList = response.jobs;
      jobsTotal = response.total;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load jobs';
    } finally {
      loading = false;
    }
  };

  const handleJobsPageChange = (delta: number) => {
    const newPage = Math.max(0, urlJobsPage + delta);
    updateUrlParams({ page: newPage === 0 ? null : newPage });
  };

  const handleJobStatusFilterChange = (status: string) => {
    updateUrlParams({ status: status || null, page: null });
  };

  const toggleShowRoutine = () => {
    updateUrlParams({ routine: urlShowRoutine ? null : '1', page: null });
  };

  const formatJobDate = (dateStr: string): string => {
    return new Date(dateStr).toLocaleString();
  };

  const getJobStatusClass = (status: string): string => {
    switch (status) {
      case 'completed':
        return 'status-completed';
      case 'failed':
        return 'status-failed';
      case 'running':
        return 'status-running';
      default:
        return 'status-pending';
    }
  };

  const toggleJobExpanded = (jobId: string) => {
    expandedJobId = expandedJobId === jobId ? null : jobId;
  };

  const toggleGroup = (key: string) => {
    const next = new Set(expandedGroups);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedGroups = next;
  };

  const navigateToEntity = (job: Job, event: MouseEvent) => {
    event.stopPropagation();
    const route = entityRoute(job.entity_type, job.entity_id);
    if (route) goto(route);
  };

  let clearingCompleted = $state(false);
  let deletingJobId = $state<string | null>(null);

  const handleClearCompleted = async () => {
    clearingCompleted = true;
    try {
      const result = await deleteCompletedJobs();
      showMessage(result.message);
      await loadJobsList();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to clear completed jobs');
    } finally {
      clearingCompleted = false;
    }
  };

  const handleDeleteJob = async (jobId: string, event: MouseEvent) => {
    event.stopPropagation();
    deletingJobId = jobId;
    const job = jobsList.find((j) => j.id === jobId);
    const isActive = job && (job.status === 'pending' || job.status === 'running');
    try {
      if (isActive) {
        await cancelJob(jobId);
        jobsList = jobsList.map((j) =>
          j.id === jobId ? { ...j, status: 'failed', error: 'Cancelled by user' } : j
        );
        showMessage('Job cancelled');
      } else {
        await deleteJob(jobId);
        jobsList = jobsList.filter((j) => j.id !== jobId);
        showMessage('Job removed');
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to process job');
    } finally {
      deletingJobId = null;
    }
  };

  function titleCase(str: string): string {
    return str.replace(/\b[a-z]/g, (c) => c.toUpperCase());
  }

  function jobLabel(job: Job): string {
    if (job.description) return titleCase(job.description);
    return formatJobType(job.job_type);
  }

  function formatResultSummary(result: Record<string, unknown>): string[] | null {
    const parts: string[] = [];
    if (typeof result.processed === 'number' && typeof result.total === 'number') {
      parts.push(`${result.processed}/${result.total} processed`);
    } else if (typeof result.processed === 'number') {
      parts.push(`${result.processed} processed`);
    }
    if (typeof result.errors === 'number' && result.errors > 0) {
      parts.push(`${result.errors} error${result.errors !== 1 ? 's' : ''}`);
    }
    if (typeof result.skipped === 'number' && result.skipped > 0) {
      parts.push(`${result.skipped} skipped`);
    }
    return parts.length > 0 ? parts : null;
  }
</script>

{#snippet jobRow({ job }: { job: Job })}
  {@const entRoute = entityRoute(job.entity_type, job.entity_id)}
  {@const resultSummary = job.result ? formatResultSummary(job.result) : null}
  <div
    class="job-row"
    class:expanded={expandedJobId === job.id}
    onclick={() => toggleJobExpanded(job.id)}
    onkeydown={(e) => e.key === 'Enter' && toggleJobExpanded(job.id)}
    tabindex="0"
    role="button"
  >
    <div class="job-row-top">
      {#if job.display?.image_url}
        <img class="job-cover" src={job.display.image_url} alt="" loading="lazy" />
      {/if}
      <div class="job-row-content">
        <div class="job-header">
          <div class="job-header-left">
            <span class="job-label">{jobLabel(job)}</span>
            <div class="job-badges">
              <span class="job-type-badge">{formatJobType(job.job_type)}</span>
              {#if job.display?.subtitle}
                <span class="job-subtitle">{job.display.subtitle}</span>
              {/if}
            </div>
          </div>
          <div class="job-header-right">
            {#if entRoute}
              <button
                class="entity-link"
                type="button"
                onclick={(e) => navigateToEntity(job, e)}
                title="View {job.entity_type ? entityTypeLabel(job.entity_type) : 'entity'}"
              >
                {job.entity_type ? entityTypeLabel(job.entity_type) : 'View'} →
              </button>
            {/if}
            <span class="job-status {getJobStatusClass(job.status)}">{job.status}</span>
            <button
              class="dismiss-btn"
              type="button"
              onclick={(e) => handleDeleteJob(job.id, e)}
              disabled={deletingJobId === job.id}
              title="Remove this job"
            >
              <span class="material-icons-outlined">close</span>
            </button>
          </div>
        </div>

        {#if job.status === 'running' && job.progress != null && job.progress > 0}
          <div class="progress-row">
            <div class="progress-bar">
              <div class="progress-fill" style="width: {job.progress}%"></div>
            </div>
            <span class="progress-label">{job.progress}%</span>
          </div>
        {/if}

        <div class="job-meta muted">
          <MetaLine
            items={[
              `Created: ${formatJobDate(job.created_at)}`,
              job.started_at && `Started: ${formatJobDate(job.started_at)}`,
              job.completed_at && `Completed: ${formatJobDate(job.completed_at)}`
            ]}
          />
        </div>
      </div>
    </div>

    {#if job.error && expandedJobId !== job.id}
      <div class="job-error-preview">{job.error}</div>
    {/if}

    {#if expandedJobId === job.id}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="job-details" onclick={(e) => e.stopPropagation()}>
        {#if resultSummary}
          <div class="job-detail-section">
            <span class="detail-label">Summary:</span>
            <div class="result-summary"><MetaLine items={resultSummary} /></div>
          </div>
        {/if}
        {#if job.payload && Object.keys(job.payload).length > 0}
          <details class="detail-collapsible">
            <summary class="detail-label">Payload</summary>
            <pre class="detail-value">{JSON.stringify(job.payload, null, 2)}</pre>
          </details>
        {/if}
        {#if job.result}
          <details class="detail-collapsible">
            <summary class="detail-label">Result</summary>
            <pre class="detail-value">{JSON.stringify(job.result, null, 2)}</pre>
          </details>
        {/if}
        {#if job.error}
          <div class="job-detail-section error-section">
            <span class="detail-label">Error:</span>
            <pre class="detail-value error-text">{job.error}</pre>
          </div>
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

<svelte:head>
  <title>Jobs — Kobbler</title>
</svelte:head>

<div class="jobs-page page-container">
  <div class="jobs-section">
    <div class="section-header">
      <h2>Jobs</h2>
      <div class="jobs-filters">
        <button
          class="btn alt small"
          type="button"
          onclick={handleClearCompleted}
          disabled={clearingCompleted}
          title="Remove all completed and failed jobs"
        >
          {clearingCompleted ? 'Clearing...' : 'Clear History'}
        </button>
        <label class="routine-toggle">
          <input type="checkbox" checked={urlShowRoutine} onchange={toggleShowRoutine} />
          Show routine
        </label>
        <div class="jobs-filter">
          <SearchSelect
            items={jobStatusOptions}
            value={urlJobsStatus}
            onchange={handleJobStatusFilterChange}
          />
        </div>
      </div>
    </div>

    {#if loading && jobsList.length === 0}
      <div class="loading-card">Loading jobs…</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else if jobsList.length === 0}
      <div class="empty-state">
        <h3>No jobs found</h3>
        <p class="muted">
          {urlJobsStatus ? 'No jobs with the selected status.' : 'No jobs have been run yet.'}
        </p>
      </div>
    {:else}
      <div class="jobs-list">
        {#each groupedJobs as group (group.key)}
          {#if group.jobs.length === 1}
            {@render jobRow({ job: group.jobs[0] })}
          {:else}
            {@const expanded = expandedGroups.has(group.key)}
            {@const anyActive = group.jobs.some(
              (j) => j.status === 'running' || j.status === 'pending'
            )}
            {@const anyFailed = group.jobs.some((j) => j.status === 'failed')}
            {@const groupStatus = anyActive
              ? 'running'
              : anyFailed
                ? 'failed'
                : group.jobs.every((j) => j.status === 'completed' || j.status === 'failed')
                  ? 'completed'
                  : 'pending'}
            <div
              class="job-row group-row"
              class:expanded
              onclick={() => toggleGroup(group.key)}
              onkeydown={(e) => e.key === 'Enter' && toggleGroup(group.key)}
              tabindex="0"
              role="button"
              aria-expanded={expanded}
            >
              <div class="job-row-top">
                <div class="job-row-content">
                  <div class="job-header">
                    <div class="job-header-left">
                      <span class="group-chevron" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
                      <span class="job-label">{group.label}</span>
                      <div class="job-badges">
                        <span class="job-type-badge">{group.jobs.length} jobs</span>
                      </div>
                    </div>
                    <div class="job-header-right">
                      <span class="group-detail">{group.detail}</span>
                      <span class="job-status {getJobStatusClass(groupStatus)}">{groupStatus}</span>
                    </div>
                  </div>
                  <div class="job-meta muted">
                    Newest: {formatJobDate(group.jobs[0].created_at)}
                  </div>
                </div>
              </div>
            </div>
            {#if expanded}
              <div class="group-children">
                {#each group.jobs as job (job.id)}
                  {@render jobRow({ job })}
                {/each}
              </div>
            {/if}
          {/if}
        {/each}
      </div>

      <div class="jobs-pagination">
        <button
          class="btn alt small"
          type="button"
          onclick={() => handleJobsPageChange(-1)}
          disabled={urlJobsPage === 0}
        >
          Previous
        </button>
        <span class="pagination-info muted">
          Page {urlJobsPage + 1} of {totalJobPages} ({jobsTotal} total)
        </span>
        <button
          class="btn alt small"
          type="button"
          onclick={() => handleJobsPageChange(1)}
          disabled={urlJobsPage >= totalJobPages - 1}
        >
          Next
        </button>
      </div>
    {/if}
  </div>
</div>

<style>
  .jobs-section {
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

  .jobs-filters {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .routine-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--muted);
    cursor: pointer;
    white-space: nowrap;
  }

  .routine-toggle input {
    cursor: pointer;
  }

  .jobs-filter {
    width: 180px;
    flex-shrink: 0;
  }

  @media (max-width: 768px) {
    .jobs-filters {
      flex-wrap: wrap;
      width: 100%;
    }

    .jobs-filter {
      width: 100%;
    }

    .job-row {
      padding: 10px 12px;
    }

    .job-header {
      flex-direction: column;
      gap: 6px;
    }

    .job-header-right {
      align-self: flex-start;
    }

    .entity-link {
      font-size: 10px;
    }
  }

  .jobs-list {
    display: grid;
    gap: 8px;
  }

  .job-row {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 12px 16px;
    text-align: left;
    cursor: pointer;
    transition: border-color 0.15s;
    width: 100%;
    color: var(--ink);
  }

  .job-row:hover {
    border-color: var(--accent);
  }

  .job-row-top {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .job-cover {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    object-fit: cover;
    flex-shrink: 0;
    background: var(--ring);
  }

  .job-row-content {
    flex: 1;
    min-width: 0;
  }

  .job-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .job-header-left {
    min-width: 0;
  }

  .job-label {
    font-weight: 600;
    font-size: 14px;
    overflow-wrap: break-word;
    display: block;
  }

  .job-badges {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 3px;
    flex-wrap: wrap;
  }

  .job-type-badge {
    font-size: 10px;
    font-weight: 500;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--ring);
    color: var(--muted);
    white-space: nowrap;
  }

  .job-subtitle {
    font-size: 11px;
    color: var(--muted);
    overflow-wrap: break-word;
  }

  .job-header-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 1;
    flex-wrap: wrap;
  }

  .entity-link {
    font-size: 11px;
    color: var(--accent);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
    white-space: nowrap;
    font-family: inherit;
    transition: background 0.15s;
  }

  .entity-link:hover {
    background: rgba(211, 140, 90, 0.1);
  }

  .progress-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
  }

  .progress-bar {
    flex: 1;
    height: 6px;
    background: var(--ring);
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.3s ease;
  }

  .progress-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    flex-shrink: 0;
    min-width: 32px;
    text-align: right;
  }

  .dismiss-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 4px;
    opacity: 0.5;
    transition:
      opacity 0.15s,
      background 0.15s,
      color 0.15s;
  }

  .dismiss-btn:hover:not(:disabled) {
    opacity: 1;
    background: var(--danger);
    color: white;
  }

  .dismiss-btn:disabled {
    cursor: not-allowed;
    opacity: 0.3;
  }

  .dismiss-btn .material-icons-outlined {
    font-size: 16px;
  }

  .job-status {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    text-transform: capitalize;
  }

  .job-status.status-completed {
    background: var(--accent-2);
    color: white;
  }

  .job-status.status-failed {
    background: var(--danger);
    color: white;
  }

  .job-status.status-running {
    background: var(--accent);
    color: white;
  }

  .job-meta {
    font-size: 11px;
    margin-top: 6px;
    overflow-wrap: break-word;
  }

  .job-error-preview {
    font-size: 12px;
    color: var(--danger);
    margin-top: 4px;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow-wrap: break-word;
  }

  .job-details {
    margin-top: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
    display: grid;
    gap: 12px;
  }

  .job-detail-section {
    display: grid;
    gap: 4px;
  }

  .detail-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--muted);
    cursor: pointer;
  }

  .detail-collapsible {
    display: grid;
    gap: 4px;
  }

  .detail-collapsible summary {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--muted);
    cursor: pointer;
    user-select: none;
  }

  .detail-collapsible summary:hover {
    color: var(--ink);
  }

  .result-summary {
    font-size: 13px;
    font-weight: 500;
    color: var(--ink);
    padding: 6px 10px;
    background: var(--bg);
    border-radius: 8px;
  }

  .detail-value {
    font-family: monospace;
    font-size: 11px;
    background: var(--bg);
    padding: 8px;
    border-radius: 8px;
    overflow-x: auto;
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    min-width: 0;
    color: var(--ink);
  }

  .error-section .detail-value {
    border: 1px solid var(--danger);
  }

  .error-text {
    color: var(--danger);
  }

  .jobs-pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding-top: 8px;
  }

  .pagination-info {
    font-size: 12px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 16px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .empty-state p {
    margin: 0;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }

  .group-row {
    cursor: pointer;
  }

  .group-chevron {
    color: var(--muted);
    font-size: 12px;
    width: 14px;
    flex-shrink: 0;
  }

  .group-detail {
    color: var(--accent);
    font-weight: 500;
    font-size: 12px;
    white-space: nowrap;
  }

  .group-children {
    display: grid;
    gap: 8px;
    padding-left: 18px;
  }
</style>
