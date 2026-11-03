import type { SessionStatusResult } from '#lib/api/client.js';

/**
 * What to do about a stream that was lost, given what the server says of its session.
 *
 * The server rebuilds a session a restart lost under the same id, so a session it
 * still answers for is reopened as it is, at the URLs the player already had. A
 * server that cannot be reached yet may still bring it back.
 */
export function reconnectStep(result: SessionStatusResult): 'reuse' | 'mint' | 'wait' {
  if (result.kind === 'unknown') return 'wait';
  if (result.kind === 'missing' || result.status.state === 'failed') return 'mint';
  return 'reuse';
}
