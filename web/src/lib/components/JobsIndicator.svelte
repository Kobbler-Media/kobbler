<script lang="ts">
  import type { Job } from '$lib/types';
  import { goto } from '$app/navigation';
  import {
    getJobsState,
    initJobsStore,
    formatJobType,
    formatTimeAgo
  } from '$lib/stores/jobs.svelte';
  import { getStreamState } from '$lib/services/eventstream.svelte';
  import { entityRoute } from '$lib/utils/entity-routes';
  import { groupJobs } from '$lib/utils/job-groups';
  import { onMount } from 'svelte';

  const jobsState = getJobsState();
  const streamState = getStreamState();
  const groupedJobs = $derived(groupJobs(jobsState.recentJobs));
  let dropdownOpen = $state(false);
  let dropdownRef = $state<HTMLDivElement | null>(null);
  let expandedGroups = $state<Set<string>>(new Set());

  function toggleGroup(key: string) {
    const next = new Set(expandedGroups);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedGroups = next;
  }

  onMount(() => {
    initJobsStore();

    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef && !dropdownRef.contains(event.target as Node)) {
        dropdownOpen = false;
      }
    }

    document.addEventListener('click', handleClickOutside);

    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  function toggleDropdown(event: MouseEvent) {
    event.stopPropagation();
    dropdownOpen = !dropdownOpen;
  }

  function handleViewAll() {
    dropdownOpen = false;
    goto('/jobs');
  }

  function handleJobClick(job: Job) {
    const route = entityRoute(job.entity_type, job.entity_id);
    dropdownOpen = false;
    if (route) {
      goto(route);
    } else {
      goto('/jobs');
    }
  }

  function getStatusIcon(status: string): string {
    switch (status) {
      case 'completed':
        return 'M9 16.2l-3.5-3.5 1.4-1.4 2.1 2.1 5.6-5.6 1.4 1.4L9 16.2z';
      case 'failed':
        return 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z';
      case 'running':
        return 'M12 4V2A10 10 0 0 0 2 12h2a8 8 0 0 1 8-8z';
      default:
        return 'M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 18a8 8 0 1 1 8-8 8 8 0 0 1-8 8z';
    }
  }

  function getStatusClass(status: string): string {
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
  }

  function formatJobProgress(job: Job): string | null {
    if (job.status !== 'running' && job.status !== 'completed') return null;

    const result = job.result;
    if (!result) return null;

    const total = result.total as number | undefined;
    const processed = result.processed as number | undefined;
    const errors = result.errors as number | undefined;

    if (typeof total === 'number' && typeof processed === 'number') {
      let text = `${processed}/${total}`;
      if (typeof errors === 'number' && errors > 0) {
        text += `, ${errors} error${errors !== 1 ? 's' : ''}`;
      }
      return text;
    }
    return null;
  }

  function titleCase(str: string): string {
    return str.replace(/\b[a-z]/g, (c) => c.toUpperCase());
  }

  function jobLabel(job: Job): string {
    if (job.description) return titleCase(job.description);
    return formatJobType(job.job_type);
  }

  function jobHasDescription(job: Job): boolean {
    return !!job.description;
  }
</script>

{#snippet jobRow({ job }: { job: Job })}
  {@const progress = formatJobProgress(job)}
  {@const hasDesc = jobHasDescription(job)}
  <button class="job-item" type="button" onclick={() => handleJobClick(job)}>
    {#if job.display?.image_url}
      <img class="job-thumb" src={job.display.image_url} alt="" loading="lazy" />
    {:else}
      <div class="job-icon {getStatusClass(job.status)}">
        <svg
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="currentColor"
          class:spinning={job.status === 'running'}
        >
          <path d={getStatusIcon(job.status)}></path>
        </svg>
      </div>
    {/if}
    <div class="job-info">
      <div class="job-label">{jobLabel(job)}</div>
      <div class="job-meta-line">
        {#if hasDesc}
          <span class="job-type-badge">{formatJobType(job.job_type)}</span>
        {/if}
        {#if job.display?.subtitle}
          <span class="job-subtitle">{job.display.subtitle}</span>
        {/if}
        {#if progress}
          <span class="job-progress-text">{progress}</span>
        {:else if job.status === 'running' && job.progress != null && job.progress > 0}
          <span class="job-progress-text">{job.progress}%</span>
        {:else}
          <span class="job-time">{formatTimeAgo(job.created_at)}</span>
        {/if}
      </div>
      {#if job.status === 'running' && job.progress != null && job.progress > 0}
        <div class="progress-bar">
          <div class="progress-fill" style="width: {job.progress}%"></div>
        </div>
      {/if}
    </div>
    {#if job.error}
      <div class="job-error" title={job.error}>!</div>
    {/if}
  </button>
{/snippet}

<div class="jobs-indicator" bind:this={dropdownRef}>
  <button
    class="jobs-btn"
    type="button"
    aria-label="Jobs"
    onclick={toggleDropdown}
    class:has-running={jobsState.runningCount > 0}
  >
    {#if jobsState.runningCount > 0}
      <svg class="spinner" viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
        <path d="M12 4V2A10 10 0 0 0 2 12h2a8 8 0 0 1 8-8z"></path>
      </svg>
      <span class="running-count">{jobsState.runningCount}</span>
    {:else}
      <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
        <path
          d="M20 6h-4V4c0-1.1-.9-2-2-2h-4c-1.1 0-2 .9-2 2v2H4c-1.1 0-2 .9-2 2v11c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zM10 4h4v2h-4V4zm10 15H4V8h16v11z"
        ></path>
      </svg>
    {/if}
  </button>

  {#if dropdownOpen}
    <div class="dropdown">
      <div class="dropdown-header">
        <span class="dropdown-title">Recent Jobs</span>
        {#if streamState.connected}
          <span class="live-indicator" title="Connected to live updates"></span>
        {/if}
      </div>

      {#if jobsState.recentJobs.length === 0}
        <div class="empty-state">No jobs yet</div>
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
              <button
                class="job-item group-summary"
                type="button"
                aria-expanded={expanded}
                onclick={() => toggleGroup(group.key)}
              >
                <div
                  class="job-icon {anyActive
                    ? 'status-running'
                    : anyFailed
                      ? 'status-failed'
                      : 'status-completed'}"
                >
                  <svg
                    viewBox="0 0 24 24"
                    width="14"
                    height="14"
                    fill="currentColor"
                    class:spinning={anyActive}
                  >
                    <path
                      d={getStatusIcon(anyActive ? 'running' : anyFailed ? 'failed' : 'completed')}
                    ></path>
                  </svg>
                </div>
                <div class="job-info">
                  <div class="job-label">{group.label}</div>
                  <div class="job-meta-line">
                    <span class="job-progress-text">{group.detail}</span>
                    <span class="job-time">{formatTimeAgo(group.jobs[0].created_at)}</span>
                  </div>
                </div>
                <span class="group-chevron" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
              </button>
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
      {/if}

      <button class="view-all-btn" type="button" onclick={handleViewAll}> View all jobs </button>
    </div>
  {/if}
</div>

<style>
  .jobs-indicator {
    position: relative;
  }

  .jobs-btn {
    border-radius: 12px;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    position: relative;
    gap: 4px;
    transition:
      width 0.2s ease,
      padding 0.2s ease,
      border-color 0.2s ease;
  }

  .jobs-btn.has-running {
    width: auto;
    padding: 0 10px;
    border-radius: 16px;
    border-color: var(--accent);
    gap: 6px;
  }

  .running-count {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
  }

  .spinner {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 8px;
    min-width: 320px;
    max-width: 380px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    z-index: 100;
    overflow: hidden;
  }

  .dropdown-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--ring);
  }

  .dropdown-title {
    font-weight: 600;
    font-size: 13px;
  }

  .live-indicator {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #22c55e;
    animation: pulse 2s infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }

  .empty-state {
    padding: 20px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }

  .jobs-list {
    max-height: 360px;
    overflow-y: auto;
  }

  .job-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--ring);
    width: 100%;
    background: transparent;
    border-left: none;
    border-right: none;
    border-top: none;
    color: var(--ink);
    text-align: left;
    cursor: pointer;
    font-family: inherit;
    transition: background 0.1s;
  }

  .job-item:last-child {
    border-bottom: none;
  }

  .job-item:hover {
    background: rgba(211, 140, 90, 0.06);
  }

  .job-thumb {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
    background: var(--ring);
  }

  .job-icon {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .job-icon.status-completed {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .job-icon.status-failed {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  .job-icon.status-running {
    background: rgba(59, 130, 246, 0.15);
    color: #3b82f6;
  }

  .job-icon.status-pending {
    background: rgba(156, 163, 175, 0.15);
    color: #9ca3af;
  }

  .spinning {
    animation: spin 1s linear infinite;
  }

  .job-info {
    flex: 1;
    min-width: 0;
  }

  .job-label {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.3;
  }

  .job-meta-line {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 2px;
    font-size: 11px;
    color: var(--muted);
    overflow: hidden;
  }

  .job-type-badge {
    font-size: 10px;
    font-weight: 500;
    padding: 0 5px;
    border-radius: 4px;
    background: var(--ring);
    color: var(--muted);
    white-space: nowrap;
    line-height: 1.6;
    flex-shrink: 0;
  }

  .job-subtitle {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .job-time {
    white-space: nowrap;
    flex-shrink: 0;
  }

  .job-progress-text {
    color: var(--accent);
    font-weight: 500;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .progress-bar {
    height: 3px;
    background: var(--ring);
    border-radius: 2px;
    margin-top: 4px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.3s ease;
  }

  .job-error {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
    font-size: 12px;
    font-weight: 700;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: help;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .view-all-btn {
    width: 100%;
    padding: 12px;
    border: none;
    border-top: 1px solid var(--ring);
    background: transparent;
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    text-align: center;
    font-family: inherit;
  }

  .view-all-btn:hover {
    background: rgba(211, 140, 90, 0.1);
  }

  .group-summary {
    cursor: pointer;
  }

  .group-chevron {
    flex-shrink: 0;
    color: var(--muted);
    font-size: 12px;
    align-self: center;
  }

  .group-children {
    padding: 0 0 4px 14px;
  }
</style>
