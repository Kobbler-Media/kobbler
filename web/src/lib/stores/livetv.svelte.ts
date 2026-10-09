import type { LivetvLease, LivetvTuner } from '$lib/types';
import type { TunerLease } from '$lib/components/livetv/types';
import { getLivetvLeases, getLivetvTuners } from '$lib/api/client';

let tuners = $state<LivetvTuner[]>([]);
let leases = $state<LivetvLease[]>([]);
let loaded = $state(false);
let lastFetch = 0;
let inflight: Promise<void> | null = null;
let leaseInflight: Promise<void> | null = null;

const CACHE_MS = 30_000;

function toTunerLease(lease: LivetvLease): TunerLease {
  return {
    id: lease.id,
    kind: lease.recording_id || lease.holder_kind === 'recording' ? 'recording' : 'watching',
    label: lease.channel_name
  };
}

export function getLivetvState() {
  return {
    get tuners() {
      return tuners;
    },
    get loaded() {
      return loaded;
    },
    get tunerCount() {
      return tuners.reduce((n, t) => n + (t.tuner_count ?? 0), 0);
    },
    get leasesInUse() {
      if (leases.length > 0) return leases.length;
      return tuners.reduce((n, t) => n + (t.leases_in_use ?? 0), 0);
    },
    get primary() {
      return tuners[0] ?? null;
    },
    get leases() {
      return leases;
    },
    get leaseLines(): TunerLease[] {
      return leases.map(toTunerLease);
    }
  };
}

async function doFetch(): Promise<void> {
  try {
    const result = await getLivetvTuners();
    tuners = result.items ?? [];
    lastFetch = Date.now();
    loaded = true;
  } catch {
    // A missing or unreachable tuner endpoint is not an error worth surfacing in
    // the sidebar — the meter simply stays empty.
    tuners = [];
    loaded = true;
  }
}

export function fetchLivetvTuners(force = false): Promise<void> {
  if (inflight) return inflight;
  if (!force && loaded && Date.now() - lastFetch < CACHE_MS) return Promise.resolve();
  inflight = doFetch().finally(() => {
    inflight = null;
  });
  return inflight;
}

export function setLivetvTuners(items: LivetvTuner[]) {
  tuners = items;
  lastFetch = Date.now();
  loaded = true;
}

async function doFetchLeases(): Promise<void> {
  try {
    const result = await getLivetvLeases();
    leases = result.items ?? [];
  } catch {
    leases = [];
  }
}

export function fetchLivetvLeases(): Promise<void> {
  if (leaseInflight) return leaseInflight;
  leaseInflight = doFetchLeases().finally(() => {
    leaseInflight = null;
  });
  return leaseInflight;
}
