import type { Job } from '$lib/types';

/**
 * Coarse job-type family used to group fan-out job rows. One download of N
 * files creates ~4N rows (probe → extract → analyze → skip_detect per file);
 * the `media_` prefix is stripped so `media_probe`/`media_extract`/... share
 * families, and jobs with the same entity stay grouped with their pipeline.
 * Unrelated types keep their own family and only group with same-type rows.
 */
export function jobFamily(jobType: string): string {
  return jobType.startsWith('media_') ? jobType.slice('media_'.length) : jobType;
}

export type JobGroup = {
  key: string;
  label: string;
  detail: string | null;
  jobs: Job[];
};

function titleCase(str: string): string {
  return str.replace(/\b[a-z]/g, (c) => c.toUpperCase());
}

/**
 * Group jobs by (entity_type, entity_id) + job-type family, preserving input
 * order. Multi-job groups render as collapsible summary rows; single-job
 * groups render as regular rows (components decide via group.jobs.length).
 */
export function groupJobs(jobs: Job[]): JobGroup[] {
  const groups = new Map<string, JobGroup>();

  for (const job of jobs) {
    const key = `${job.entity_type ?? ''}|${job.entity_id ?? ''}|${jobFamily(job.job_type)}`;
    let group = groups.get(key);
    if (!group) {
      group = { key, label: '', detail: null, jobs: [] };
      groups.set(key, group);
    }
    group.jobs.push(job);
  }

  for (const group of groups.values()) {
    if (group.jobs.length < 2) continue;

    const [first] = group.jobs;
    const sharedDesc = group.jobs.every(
      (j) => j.description !== null && j.description === first.description
    )
      ? first.description
      : null;
    group.label = sharedDesc ? titleCase(sharedDesc) : titleCase(jobFamily(first.job_type));

    const done = group.jobs.filter((j) => j.status === 'completed' || j.status === 'failed').length;
    const failed = group.jobs.filter((j) => j.status === 'failed').length;
    group.detail = `${done}/${group.jobs.length} done${failed > 0 ? ` · ${failed} failed` : ''}`;
  }

  return [...groups.values()];
}
