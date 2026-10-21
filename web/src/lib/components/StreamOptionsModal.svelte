<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import type { StreamOption } from '$lib/api/client';

  interface Props {
    options: StreamOption[];
    loading: boolean;
    errors: string[];
    onClose: () => void;
    onPlay: (option: StreamOption) => void;
  }

  let { options, loading, errors, onClose, onPlay }: Props = $props();

  function formatSize(bytes: number): string {
    if (!bytes) return 'Unknown';
    const mb = bytes / 1024 / 1024;
    if (mb < 1024) return `${mb.toFixed(1)} MB`;
    return `${(mb / 1024).toFixed(2)} GB`;
  }

  function formatScore(score: number): string {
    return `${Math.round(score * 100)}%`;
  }

  function scoreClass(score: number): string {
    if (score >= 0.8) return 'score-high';
    if (score >= 0.5) return 'score-mid';
    return 'score-low';
  }

  function getResolution(quality: Record<string, unknown> | undefined): string | null {
    if (!quality) return null;
    const res = quality.resolution ?? quality.source;
    return typeof res === 'string' ? res : null;
  }

  function getCodec(quality: Record<string, unknown> | undefined): string | null {
    if (!quality) return null;
    const codec = quality.codec ?? quality.video_codec;
    return typeof codec === 'string' ? codec : null;
  }

  function getHdr(quality: Record<string, unknown> | undefined): string | null {
    if (!quality) return null;
    const hdr = quality.hdr ?? quality.dynamic_range;
    if (typeof hdr === 'string' && hdr.toLowerCase() !== 'sdr') return hdr;
    return null;
  }

  function getAudio(quality: Record<string, unknown> | undefined): string | null {
    if (!quality) return null;
    const audio = quality.audio ?? quality.audio_codec;
    return typeof audio === 'string' ? audio : null;
  }

  /** Extract a short source label from the indexer field (e.g., "hash_index:rarbg_dump" -> "RARBG") */
  function getSourceLabel(indexer: string): string | null {
    if (!indexer.startsWith('hash_index:')) return null;
    const source = indexer.slice('hash_index:'.length);
    const labels: Record<string, string> = {
      rarbg_dump: 'RARBG',
      tpb: 'TPB',
      dmm: 'DMM',
      stremthru: 'StremThru',
      torrentio: 'Torrentio',
      eztv: 'EZTV',
      dht: 'DHT',
      rss: 'RSS',
      live_search: 'Live'
    };
    return labels[source] ?? source;
  }
</script>

<Modal {onClose} title="Stream Options" width="600px" overflowVisible>
  <div class="modal-content">
    {#if loading}
      <div class="empty-state">
        <span class="material-icons-outlined spin">refresh</span>
        <span>Searching for streams...</span>
      </div>
    {:else if errors.length > 0 && options.length === 0}
      <div class="error-section">
        {#each errors as error, i (i)}
          <div class="error-message">{error}</div>
        {/each}
      </div>
    {:else if options.length === 0}
      <div class="empty-state">
        <span class="material-icons-outlined">cloud_off</span>
        <span>No streams found</span>
      </div>
    {:else}
      {#if errors.length > 0}
        <div class="error-section compact">
          {#each errors as error, i (i)}
            <div class="error-message">{error}</div>
          {/each}
        </div>
      {/if}
      <div class="results-list">
        {#each options as option (option.id)}
          {@const resolution = getResolution(option.quality)}
          {@const codec = getCodec(option.quality)}
          {@const hdr = getHdr(option.quality)}
          {@const audio = getAudio(option.quality)}
          {@const sourceLabel = getSourceLabel(option.indexer)}
          <button type="button" class="stream-item" onclick={() => onPlay(option)}>
            <div class="stream-info">
              <div class="stream-title-row">
                <span class="stream-title">{option.release_title}</span>
                <span class="score-pill {scoreClass(option.total_score)}"
                  >{formatScore(option.total_score)}</span
                >
              </div>
              <div class="stream-badges">
                {#if sourceLabel}
                  <span class="badge badge-source">{sourceLabel}</span>
                {/if}
                <!-- Only claim either end when the backend actually knows it:
                     "likely" and "unknown" stay unlabelled rather than guessed. -->
                {#if option.cached || option.availability === 'ready'}
                  <span class="badge badge-instant">
                    <span class="material-icons-outlined badge-icon">bolt</span>
                    Instant
                  </span>
                {:else if option.availability === 'uncached'}
                  <span class="badge badge-uncached">Needs download</span>
                {/if}
                {#if option.playback === 'worked'}
                  <span class="badge badge-worked">Played before</span>
                {:else if option.playback === 'failed'}
                  <span class="badge badge-failed">Failed before</span>
                {/if}
                {#if resolution}
                  <span class="badge badge-quality">{resolution}</span>
                {/if}
                {#if codec}
                  <span class="badge badge-codec">{codec}</span>
                {/if}
                {#if hdr}
                  <span class="badge badge-hdr">{hdr}</span>
                {/if}
                {#if audio}
                  <span class="badge badge-audio">{audio}</span>
                {/if}
              </div>
              <div class="stream-meta">
                <span class="meta-size">{formatSize(option.size)}</span>
                <span class="meta-indexer">{option.indexer}</span>
                <span class="meta-debrid">{option.debrid_provider}</span>
                {#if option.source_type === 'usenet'}
                  <span class="meta-source">Usenet</span>
                {/if}
              </div>
            </div>
            <span class="material-icons-outlined play-icon">play_circle</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</Modal>

<style>
  .modal-content {
    display: flex;
    flex-direction: column;
    max-height: calc(90vh - 100px);
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    padding: 32px;
    font-size: 13px;
  }

  .empty-state .material-icons-outlined {
    font-size: 32px;
    opacity: 0.5;
  }

  .error-section {
    margin-bottom: 12px;
  }

  .error-section.compact {
    margin-bottom: 8px;
  }

  .error-message {
    color: var(--destructive);
    padding: 8px;
    margin-bottom: 4px;
    border: 1px solid var(--destructive);
    border-radius: 8px;
    font-size: 12px;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 400px;
    overflow-y: auto;
  }

  .stream-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .stream-item:hover {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 5%, var(--card));
  }

  .stream-info {
    flex: 1;
    min-width: 0;
  }

  .stream-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .stream-title {
    font-size: 13px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .stream-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    line-height: 1.3;
  }

  .badge-icon {
    font-size: 12px;
  }

  .badge-instant {
    background: color-mix(in srgb, var(--primary) 15%, transparent);
    color: var(--primary);
  }

  .badge-worked {
    background: color-mix(in srgb, var(--primary) 15%, transparent);
    color: var(--primary);
  }

  .badge-uncached {
    background: color-mix(in srgb, var(--ink) 10%, transparent);
    color: var(--muted);
  }

  .badge-failed {
    background: color-mix(in srgb, var(--destructive) 15%, transparent);
    color: var(--destructive);
  }

  .badge-quality {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
  }

  .badge-codec,
  .badge-audio {
    background: color-mix(in srgb, var(--ink) 10%, transparent);
    color: var(--ink);
    opacity: 0.8;
  }

  .badge-hdr {
    background: color-mix(in srgb, #fbbf24 20%, transparent);
    color: #fbbf24;
  }

  .badge-source {
    background: color-mix(in srgb, var(--primary) 12%, transparent);
    color: var(--primary);
    font-size: 10px;
    letter-spacing: 0.5px;
    text-transform: uppercase;
  }

  .stream-meta {
    display: flex;
    gap: 12px;
    margin-top: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .play-icon {
    font-size: 28px;
    color: var(--accent);
    flex-shrink: 0;
    opacity: 0.6;
    transition: opacity 0.15s;
  }

  .stream-item:hover .play-icon {
    opacity: 1;
  }

  /* Score pills */
  .score-pill {
    flex-shrink: 0;
    display: inline-block;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    line-height: 1.3;
  }

  .score-high {
    background: color-mix(in srgb, var(--primary) 15%, transparent);
    color: var(--primary);
  }

  .score-mid {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
  }

  .score-low {
    background: color-mix(in srgb, var(--destructive) 15%, transparent);
    color: var(--destructive);
  }

  /* Loading spinner */
  .spin {
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
</style>
