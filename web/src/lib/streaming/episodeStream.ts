import { createPlaybackSession, streamResolve, streamSearch } from '#lib/api/client.js';
import type { PlaybackSessionResponse } from '#lib/types.js';
import { detectCapabilities, type ClientCapabilities } from '#lib/utils/media.js';
import { createPrewarm } from './prewarm';
import { rankOptions, resolveFirstReady } from './walk';

const NEXT_PREWARM_AT = 0.7;

async function resolveEpisodeStream(episodeId: string): Promise<PlaybackSessionResponse | null> {
  const options = rankOptions(await streamSearch({ episode_id: episodeId }));
  return resolveFirstReady(options, (opt) =>
    streamResolve({
      debrid_provider_id: opt.debrid_provider_id,
      info_hash: opt.info_hash,
      file_id: opt.files[0]?.id,
      media_type: 'episode',
      media_id: episodeId,
      release_title: opt.release_title,
      debrid_provider: opt.debrid_provider,
      quality: opt.quality,
      capabilities: detectCapabilities()
    })
  );
}

const episodeParams = (episodeId: string) => ({
  media_type: 'episode',
  media_id: episodeId,
  capabilities: detectCapabilities()
});

interface EpisodeTarget {
  episode_id: string;
  file_id: string | null;
}

export function createNextEpisodeWarmer() {
  const prewarm = createPrewarm();
  let warmedFor: string | null = null;

  function consider(next: EpisodeTarget | null, fraction: number) {
    if (!next || next.file_id || fraction < NEXT_PREWARM_AT || warmedFor === next.episode_id) {
      return;
    }
    warmedFor = next.episode_id;
    prewarm.start(episodeParams(next.episode_id), 0);
  }

  async function streamFor(episodeId: string): Promise<PlaybackSessionResponse | null> {
    warmedFor = null;
    const claimed = await prewarm.claim(episodeParams(episodeId));
    return claimed?.session ?? resolveEpisodeStream(episodeId);
  }

  function release(keepalive = false) {
    warmedFor = null;
    prewarm.release(keepalive);
  }

  async function open(
    target: EpisodeTarget,
    capabilities: ClientCapabilities
  ): Promise<{ session: PlaybackSessionResponse; fileId: string } | null> {
    if (target.file_id) {
      release();
      const session = await createPlaybackSession({
        media_type: 'episode',
        file_id: target.file_id,
        capabilities
      });
      return { session, fileId: target.file_id };
    }
    const session = await streamFor(target.episode_id);
    return session ? { session, fileId: `debrid-${session.session_id}` } : null;
  }

  return { consider, open, release };
}
