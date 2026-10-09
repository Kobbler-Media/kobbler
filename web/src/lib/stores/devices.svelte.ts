// Devices and activity store using Svelte 5 runes

import type { DeviceSummary, ActivityEvent, UserSummary, IngestTokenSummary } from '$lib/types';
import type { ProxyLogEntry } from '$lib/api/client';

let devices = $state.raw<DeviceSummary[]>([]);
let activityEvents = $state.raw<ActivityEvent[]>([]);
let activityNextCursor = $state<string | null>(null);
let activityTotal = $state<number>(0);
let users = $state.raw<UserSummary[]>([]);
let ingestTokens = $state.raw<IngestTokenSummary[]>([]);
let createdIngestToken = $state('');
let proxyLogs = $state.raw<ProxyLogEntry[]>([]);
let proxyLogsTotal = $state(0);
let proxyLogsOffset = $state(0);

export function getDevicesState() {
  return {
    get devices() {
      return devices;
    },
    get activityEvents() {
      return activityEvents;
    },
    get activityNextCursor() {
      return activityNextCursor;
    },
    get activityTotal() {
      return activityTotal;
    },
    get users() {
      return users;
    },
    get ingestTokens() {
      return ingestTokens;
    },
    get createdIngestToken() {
      return createdIngestToken;
    },
    get proxyLogs() {
      return proxyLogs;
    },
    get proxyLogsTotal() {
      return proxyLogsTotal;
    },
    get proxyLogsOffset() {
      return proxyLogsOffset;
    }
  };
}

export function setDevices(newDevices: DeviceSummary[]) {
  devices = newDevices;
}

export function setActivityEvents(
  events: ActivityEvent[],
  nextCursor: string | null,
  total?: number,
  mode: 'replace' | 'append' | 'prepend' = 'replace'
) {
  if (mode === 'append') {
    activityEvents = [...activityEvents, ...events];
  } else if (mode === 'prepend') {
    activityEvents = [...events, ...activityEvents];
  } else {
    activityEvents = events;
  }
  activityNextCursor = nextCursor;
  if (total !== undefined) {
    activityTotal = total;
  }
}

export function setUsers(newUsers: UserSummary[]) {
  users = newUsers;
}

export function setIngestTokens(tokens: IngestTokenSummary[]) {
  ingestTokens = tokens;
}

export function setCreatedIngestToken(token: string) {
  createdIngestToken = token;
}

export function setProxyLogs(logs: ProxyLogEntry[], total: number, offset: number) {
  proxyLogs = logs;
  proxyLogsTotal = total;
  proxyLogsOffset = offset;
}

export function clearDevicesState() {
  devices = [];
  activityEvents = [];
  activityNextCursor = null;
  activityTotal = 0;
  users = [];
  ingestTokens = [];
  createdIngestToken = '';
  proxyLogs = [];
  proxyLogsTotal = 0;
  proxyLogsOffset = 0;
}
