import { streamResolve, streamSearch } from '$lib/api/client';
import type { PlaybackSessionResponse } from '$lib/types';
import { detectCapabilities } from '$lib/utils/media';
import { rankOptions, resolveFirstReady } from './walk';

export async function resolveEpisodeStream(
  episodeId: string
): Promise<PlaybackSessionResponse | null> {
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
