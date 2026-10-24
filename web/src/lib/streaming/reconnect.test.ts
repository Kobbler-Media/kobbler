import { describe, expect, it } from 'vitest';
import type { SessionStatusResult } from '$lib/api/client';
import { reconnectStep } from './reconnect';

function status(state: 'ready' | 'preparing' | 'failed'): SessionStatusResult {
  return {
    kind: 'status',
    status: { session_id: 's', state, method: 'remux', is_live: false }
  };
}

describe('reconnectStep', () => {
  it.each<[string, SessionStatusResult, ReturnType<typeof reconnectStep>]>([
    ['reopens the session a restart brought back', status('ready'), 'reuse'],
    ['reopens a session still preparing', status('preparing'), 'reuse'],
    ['mints a new session when the old one failed', status('failed'), 'mint'],
    ['mints a new session when the server has no record of it', { kind: 'missing' }, 'mint'],
    ['waits while the server cannot be reached', { kind: 'unknown' }, 'wait']
  ])('%s', (_, result, expected) => {
    expect(reconnectStep(result)).toBe(expected);
  });
});
