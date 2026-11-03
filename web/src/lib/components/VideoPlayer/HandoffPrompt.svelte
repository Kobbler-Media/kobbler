<script lang="ts">
  import type { WatchProgress } from '#lib/types.js';

  interface Props {
    progress: WatchProgress;
    /** True when the remote position is in a different episode than this player's. */
    otherEpisode: boolean;
    /** Seconds until "stay here" is chosen for the viewer, or null when not counting. */
    stayCountdown: number | null;
    onJump: () => void;
    onStay: () => void;
  }

  let { progress, otherEpisode, stayCountdown, onJump, onStay }: Props = $props();

  function clock(total: number): string {
    const s = Math.max(0, Math.floor(total));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = String(s % 60).padStart(2, '0');
    return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
  }

  const device = $derived(progress.client_label || 'another device');
  const ep = $derived(progress.episode);
  const code = $derived(ep ? `S${ep.season_identifier}E${ep.episode_identifier}` : null);
  const title = $derived(
    ep ? ep.title || `Episode ${ep.episode_identifier}` : progress.movie_title || 'This movie'
  );
  const where = $derived(
    progress.watched ? 'Finished' : `Stopped at ${clock(progress.position_seconds)}`
  );
  const jumpLabel = $derived(
    otherEpisode && code ? `Go to ${code}` : `Jump to ${clock(progress.position_seconds)}`
  );
</script>

<div class="handoff-overlay" role="dialog" aria-label="Continue from {device}">
  <div class="handoff-card">
    {#if ep?.still_url && otherEpisode}
      <img class="handoff-thumb" src={ep.still_url} alt="" />
    {:else}
      <span class="material-icons-outlined handoff-icon" aria-hidden="true">devices</span>
    {/if}
    <div class="handoff-info">
      <span class="handoff-label">Watched on {device}</span>
      <span class="handoff-title">{code ? `${code} · ${title}` : title}</span>
      <span class="handoff-meta">{where}</span>
    </div>
    <div class="handoff-actions">
      <button class="jump-btn" type="button" onclick={onJump}>{jumpLabel}</button>
      <button class="stay-btn" type="button" onclick={onStay}>
        {stayCountdown != null ? `Stay here (${stayCountdown})` : 'Stay here'}
      </button>
    </div>
  </div>
</div>

<style>
  .handoff-overlay {
    position: absolute;
    bottom: calc(100px + env(safe-area-inset-bottom));
    right: calc(24px + env(safe-area-inset-right));
    z-index: 10;
    max-width: calc(100% - 48px);
  }

  .handoff-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    background: rgba(0, 0, 0, 0.85);
    border-radius: 8px;
    backdrop-filter: blur(12px);
    border: 1px solid rgba(255, 255, 255, 0.1);
    max-width: 420px;
  }

  .handoff-thumb {
    width: 120px;
    height: 68px;
    object-fit: cover;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .handoff-icon {
    font-size: 32px;
    color: rgba(255, 255, 255, 0.7);
    flex-shrink: 0;
    padding: 0 4px;
  }

  .handoff-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .handoff-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: rgba(255, 255, 255, 0.5);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .handoff-title {
    font-size: 14px;
    font-weight: 600;
    color: white;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .handoff-meta {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
  }

  .handoff-actions {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: stretch;
    flex-shrink: 0;
  }

  .jump-btn {
    padding: 6px 16px;
    border: none;
    border-radius: 4px;
    background: var(--accent, #818cf8);
    color: white;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .jump-btn:hover {
    filter: brightness(1.1);
  }

  .stay-btn {
    padding: 4px 12px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: rgba(255, 255, 255, 0.6);
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
  }

  .stay-btn:hover {
    color: white;
  }
</style>
