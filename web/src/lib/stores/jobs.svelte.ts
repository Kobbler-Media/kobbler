// Jobs store using Svelte 5 runes — driven by unified SSE events via layout.

import type { Job, JobEvent } from '#lib/types.js';
import { getJobs, getRunningJobsCount } from '#lib/api/client.js';
import { getStreamState } from '#lib/services/eventstream.svelte.js';

// State
let jobs = $state.raw<Job[]>([]);
let total = $state<number>(0);
let serverRunningCount = $state<number>(0);
let countInitialized = $state(false);

// Local count from the cached job list — pre-init fallback only
const localRunningCount = $derived(
  jobs.filter((j) => (j.status === 'pending' || j.status === 'running') && !j.scheduled).length
);

// Server-authoritative once initialized; before the first fetch the local
// list is the only signal available
const runningCount = $derived(
  countInitialized ? serverRunningCount : Math.max(localRunningCount, serverRunningCount)
);

// Derived: recent jobs excludes completed/pending routine jobs
const recentJobs = $derived(
  jobs
    .filter((j) => !(j.scheduled && (j.status === 'completed' || j.status === 'pending')))
    .slice(0, 10)
);

// Getters
export function getJobsState() {
  return {
    get jobs() {
      return jobs;
    },
    get runningCount() {
      return runningCount;
    },
    get total() {
      return total;
    },
    get recentJobs() {
      return recentJobs;
    }
  };
}

// Initial hydration only — called once on connect
export async function loadJobs(limit = 50, offset = 0) {
  const response = await getJobs({ limit, offset });
  jobs = response.jobs;
  total = response.total;
}

// Debounce buffer — plain (non-reactive) to avoid triggering updates per event
let eventBuffer: JobEvent[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;

// Statuses that end a job's contribution to the running count
const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled']);

function flushJobEvents() {
  flushTimer = null;
  if (eventBuffer.length === 0) return;

  const pending = eventBuffer;
  eventBuffer = [];

  // Apply all buffered events in one pass
  let updated = jobs;
  let newTotal = total;
  let hasTerminal = false;

  for (const event of pending) {
    if (TERMINAL_STATUSES.has(event.status)) hasTerminal = true;

    const jobIndex = updated.findIndex((j) => j.id === event.job_id);

    if (jobIndex >= 0) {
      const prev = updated[jobIndex];

      const merged = {
        ...prev,
        status: event.status as Job['status'],
        progress: event.progress,
        error: event.error,
        scheduled: event.scheduled ?? prev.scheduled,
        ...(event.description != null && { description: event.description }),
        ...(event.display != null && { display: event.display }),
        ...(event.result != null && { result: event.result })
      };
      updated = [...updated.slice(0, jobIndex), merged, ...updated.slice(jobIndex + 1)];
    } else {
      const newJob: Job = {
        id: event.job_id,
        job_type: event.job_type,
        status: event.status as Job['status'],
        payload: {},
        result: event.result ?? null,
        error: event.error,
        progress: event.progress,
        created_at: event.created_at,
        started_at: event.status === 'running' ? event.created_at : null,
        completed_at:
          event.status === 'completed' || event.status === 'failed'
            ? new Date().toISOString()
            : null,
        description: event.description,
        entity_type: event.entity_type,
        entity_id: event.entity_id,
        display: event.display ?? null,
        scheduled: event.scheduled ?? false
      };
      updated = [newJob, ...updated];
      newTotal += 1;
    }
  }

  // Single reactive write for the entire batch
  jobs = updated;
  total = newTotal;

  // Any terminal transition means the running count changed — refetch it
  // from the server (debounced) instead of trusting deltas.
  if (hasTerminal) scheduleCountRefetch();

  // Notify page-level listeners
  for (const fn of changeListeners) fn();
}

// Change listeners — notified after each SSE flush
type ChangeListener = () => void;
let changeListeners: ChangeListener[] = [];

export function onJobChange(fn: ChangeListener): () => void {
  changeListeners.push(fn);
  return () => {
    changeListeners = changeListeners.filter((l) => l !== fn);
  };
}

/** Handle a job event from the unified SSE stream. Called by the layout. */
export function handleJobEvent(event: JobEvent) {
  eventBuffer.push(event);
  if (!flushTimer) {
    flushTimer = setTimeout(flushJobEvents, 100);
  }
}

// Debounced refetch after terminal transitions — the server count replaces
// (never deltas) the stored value so a missed event can't pin it high.
let countRefetchTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleCountRefetch() {
  if (countRefetchTimer) return;
  countRefetchTimer = setTimeout(() => {
    countRefetchTimer = null;
    void syncRunningCount();
  }, 500);
}

/** Fetch the running job count from the server and replace the stored value. */
export async function syncRunningCount() {
  try {
    const result = await getRunningJobsCount();
    serverRunningCount = result.count;
    countInitialized = true;
  } catch {
    // Ignore — the backstop poll or a reconnect re-sync will retry
  }
}

// Backstop: while the SSE stream is connected, re-poll the count every 15s so
// any drift (missed events, races) self-corrects within one interval.
let backstopTimer: ReturnType<typeof setInterval> | null = null;

function ensureBackstopPoll() {
  if (backstopTimer) return;
  backstopTimer = setInterval(() => {
    if (getStreamState().connected) void syncRunningCount();
  }, 15_000);
}

// Initialize — call this when the app loads
export async function initJobsStore() {
  ensureBackstopPoll();
  try {
    const [, countResult] = await Promise.all([
      loadJobs(10),
      getRunningJobsCount().catch(() => ({ count: 0 }))
    ]);
    serverRunningCount = countResult.count;
    countInitialized = true;
  } catch {
    // Ignore errors on init — user might not be logged in
  }
}

// Format job type for display
export function formatJobType(jobType: string): string {
  return jobType
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}

// Format time ago
export function formatTimeAgo(dateStr: string): string {
  const dateMs = Date.parse(dateStr);
  const nowMs = Date.now();
  const seconds = Math.floor((nowMs - dateMs) / 1000);

  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}
