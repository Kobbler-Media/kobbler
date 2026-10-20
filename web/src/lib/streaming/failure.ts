import type { StreamFailure } from './walk';

export const PLAYER_FAILURE_REASON = 'wont_play';
export const CACHE_FAILURE_REASON = 'download_failed';

const REASON_SENTENCES: Record<string, string> = {
  blocked: 'Your debrid service refused the release.',
  rate_limited: 'Your debrid service is limiting requests right now.',
  auth_failed: 'Your debrid service rejected its credentials. Check the API keys in Settings.',
  not_cached: "The release isn't available on your debrid service.",
  timeout: 'Your debrid service took too long to respond.',
  unavailable: 'The stream service is unavailable right now.',
  not_video: "The release doesn't contain a playable video.",
  [PLAYER_FAILURE_REASON]: "Your browser couldn't play them.",
  [CACHE_FAILURE_REASON]: 'The download failed on your debrid service.'
};

function dominantFailure(failures: StreamFailure[]): StreamFailure | undefined {
  const counts = new Map<string, number>();
  let best: StreamFailure | undefined;
  let bestCount = 0;
  for (const failure of failures) {
    const key = failure.reason ?? failure.message;
    const count = (counts.get(key) ?? 0) + 1;
    counts.set(key, count);
    if (count > bestCount) {
      best = failure;
      bestCount = count;
    }
  }
  return best;
}

export function explainFailure(failure: StreamFailure): string {
  return (failure.reason && REASON_SENTENCES[failure.reason]) || failure.message;
}

export function summarizeFailures(failures: StreamFailure[], tried: number): string {
  const lead =
    tried <= 1
      ? "The stream we tried wouldn't play."
      : `None of the ${tried} streams we tried would play.`;
  const dominant = dominantFailure(failures);
  return dominant ? `${lead} ${explainFailure(dominant)}` : lead;
}

export function summarizeNoStreams(searchErrors: string[]): string {
  return searchErrors[0] ?? "We couldn't find a stream for this title.";
}

export function failureDetails(failures: StreamFailure[]): string[] {
  return failures.map((f) => `${f.release ?? 'Stream'}: ${f.detail ?? f.message}`);
}
