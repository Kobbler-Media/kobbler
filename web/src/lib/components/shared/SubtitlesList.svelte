<script lang="ts">
  import type { SubtitleTrackInfo, ExternalSubtitleInfo } from '#lib/types.js';

  interface Props {
    subtitleTracks: SubtitleTrackInfo[];
    externalSubtitles: ExternalSubtitleInfo[];
    onSearchSubtitles?: () => void;
  }

  let { subtitleTracks, externalSubtitles, onSearchSubtitles }: Props = $props();

  let expanded = $state(false);

  const totalCount = $derived(subtitleTracks.length + externalSubtitles.length);

  const languageLabel = (t: { language: string | null; language_name: string | null }) =>
    t.language_name ?? t.language ?? 'Unknown';
</script>

<div class="subtitles-section">
  <button class="section-toggle" type="button" onclick={() => (expanded = !expanded)}>
    <span class="material-icons-outlined toggle-icon" class:open={expanded}>expand_more</span>
    <span class="section-label">Subtitles</span>
    <span class="section-count muted">{totalCount}</span>
    {#if onSearchSubtitles}
      <span
        class="search-btn"
        role="button"
        tabindex="0"
        title="Search for subtitles"
        onclick={(e: MouseEvent) => {
          e.stopPropagation();
          onSearchSubtitles?.();
        }}
        onkeydown={(e: KeyboardEvent) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            e.stopPropagation();
            onSearchSubtitles?.();
          }
        }}
      >
        <span class="material-icons-outlined">search</span>
      </span>
    {/if}
  </button>

  {#if expanded}
    <div class="subtitle-list">
      {#if totalCount === 0}
        <div class="empty muted">No subtitles available</div>
      {/if}

      {#each subtitleTracks as track (track.id)}
        <div class="sub-item">
          <div class="sub-info">
            <span class="sub-lang">{languageLabel(track)}</span>
            <span class="sub-codec">{track.codec}</span>
            {#if track.is_forced}
              <span class="badge forced">Forced</span>
            {/if}
            {#if track.is_hearing_impaired}
              <span class="badge hi">HI</span>
            {/if}
            {#if track.is_default}
              <span class="badge default">Default</span>
            {/if}
          </div>
          <span class="sub-source muted">Embedded</span>
        </div>
      {/each}

      {#each externalSubtitles as ext (ext.id)}
        <div class="sub-item" class:unavailable={!ext.available}>
          <div class="sub-info">
            <span class="sub-lang">{languageLabel(ext)}</span>
            <span class="sub-codec">{ext.format.toUpperCase()}</span>
            {#if ext.is_forced}
              <span class="badge forced">Forced</span>
            {/if}
            {#if ext.is_hearing_impaired}
              <span class="badge hi">HI</span>
            {/if}
          </div>
          <span class="sub-source muted">{ext.source}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .subtitles-section {
    display: flex;
    flex-direction: column;
  }

  .section-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    font-size: 14px;
    font-weight: 700;
    text-align: left;
  }

  .toggle-icon {
    font-size: 20px;
    transition: transform 0.15s;
    color: var(--muted);
  }

  .toggle-icon.open {
    transform: rotate(180deg);
  }

  .section-count {
    font-size: 12px;
    font-weight: 400;
  }

  .search-btn {
    margin-left: auto;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .search-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .search-btn .material-icons-outlined {
    font-size: 16px;
  }

  .subtitle-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 8px;
  }

  .empty {
    font-size: 12px;
    padding: 8px 0;
  }

  .sub-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 8px;
    background: var(--card);
    border: 1px solid var(--ring);
  }

  .sub-item.unavailable {
    opacity: 0.4;
  }

  .sub-info {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
    min-width: 0;
  }

  .sub-lang {
    font-size: 12px;
    font-weight: 600;
  }

  .sub-codec {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--ring);
    font-weight: 600;
  }

  .badge {
    font-size: 9px;
    padding: 1px 4px;
    border-radius: 3px;
    font-weight: 600;
  }

  .badge.forced {
    background: var(--accent-dim, rgba(232, 168, 124, 0.15));
    color: var(--accent);
  }

  .badge.hi {
    background: rgba(100, 149, 237, 0.15);
    color: cornflowerblue;
  }

  .badge.default {
    background: rgba(100, 200, 100, 0.15);
    color: mediumseagreen;
  }

  .sub-source {
    font-size: 10px;
    flex-shrink: 0;
  }
</style>
