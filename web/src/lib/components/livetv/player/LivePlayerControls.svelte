<script lang="ts">
  import RecordButton from '../RecordButton.svelte';

  interface Props {
    playing: boolean;
    atLive: boolean;
    behindLabel: string;
    jumpDisabled: boolean;
    jumpDisabledReason: string;
    canRestart: boolean;
    recording: boolean;
    recordBusy: boolean;
    recordTitle: string;
    muted: boolean;
    panelOpen: boolean;
    isFullscreen: boolean;
    onPlayPause: () => void;
    onJumpToLive: () => void;
    onRestart: () => void;
    onRecord: () => void;
    onToggleMute: () => void;
    onTogglePanel: () => void;
    onToggleFullscreen: () => void;
  }

  let {
    playing,
    atLive,
    behindLabel,
    jumpDisabled,
    jumpDisabledReason,
    canRestart,
    recording,
    recordBusy,
    recordTitle,
    muted,
    panelOpen,
    isFullscreen,
    onPlayPause,
    onJumpToLive,
    onRestart,
    onRecord,
    onToggleMute,
    onTogglePanel,
    onToggleFullscreen
  }: Props = $props();
</script>

<div class="controls">
  <button
    type="button"
    class="primary"
    onclick={onPlayPause}
    aria-label={playing ? 'Pause' : 'Play'}
  >
    <span class="material-icons-outlined">{playing ? 'pause' : 'play_arrow'}</span>
  </button>

  {#if !atLive}
    <button
      type="button"
      class="pill jump"
      disabled={jumpDisabled}
      title={jumpDisabled ? jumpDisabledReason : 'Skip forward to the live edge'}
      onclick={onJumpToLive}
    >
      <span class="material-icons-outlined">last_page</span>
      Jump to live{behindLabel ? ` · ${behindLabel}` : ''}
    </button>
  {/if}

  <button
    type="button"
    class="pill"
    disabled={!canRestart}
    title={canRestart
      ? 'Play this programme from the start of the buffer'
      : 'This programme started before you tuned in'}
    onclick={onRestart}
  >
    <span class="material-icons-outlined">restart_alt</span>
    Restart
  </button>

  <RecordButton
    {recording}
    busy={recordBusy}
    variant="overlay"
    title={recordTitle}
    onclick={onRecord}
  />

  <button
    type="button"
    class="pill muted-btn"
    disabled
    title="DVB teletext subtitles arrive with the subtitle phase"
  >
    <span class="material-icons-outlined">closed_caption_off</span>
    Teletext subs
  </button>

  <span class="spacer"></span>

  <button
    type="button"
    class="pill panel"
    class:active={panelOpen}
    onclick={onTogglePanel}
    aria-pressed={panelOpen}
    title="Browse channels without leaving the picture"
  >
    <span class="material-icons-outlined">grid_view</span>
    Guide
  </button>

  <button type="button" class="icon" onclick={onToggleMute} aria-label={muted ? 'Unmute' : 'Mute'}>
    <span class="material-icons-outlined">{muted ? 'volume_off' : 'volume_up'}</span>
  </button>

  <button
    type="button"
    class="icon"
    onclick={onToggleFullscreen}
    aria-label={isFullscreen ? 'Exit full screen' : 'Full screen'}
  >
    <span class="material-icons-outlined">
      {isFullscreen ? 'fullscreen_exit' : 'fullscreen'}
    </span>
  </button>
</div>

<style>
  .controls {
    display: flex;
    align-items: center;
    gap: 9px;
    flex-wrap: wrap;
  }

  .primary {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: 11px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    cursor: pointer;
    flex-shrink: 0;
  }

  .primary .material-icons-outlined {
    font-size: 22px;
  }

  .pill,
  .icon {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 40px;
    border-radius: 11px;
    background: rgba(0, 0, 0, 0.45);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: var(--ink);
    font-size: 12.5px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .pill {
    padding: 0 14px;
  }

  .icon {
    width: 40px;
    justify-content: center;
    padding: 0;
    flex-shrink: 0;
  }

  .pill:hover:not(:disabled),
  .icon:hover {
    background: rgba(0, 0, 0, 0.65);
    border-color: rgba(255, 255, 255, 0.22);
  }

  .pill:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .muted-btn {
    color: var(--muted);
  }

  .jump {
    background: rgba(239, 68, 68, 0.16);
    border-color: var(--danger);
    font-weight: 700;
  }

  .jump:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.3);
    border-color: var(--danger);
  }

  .panel {
    font-weight: 700;
    padding: 0 15px;
  }

  .panel.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #1d130c;
  }

  .panel.active:hover {
    background: var(--accent);
    border-color: var(--accent);
  }

  .pill .material-icons-outlined,
  .icon .material-icons-outlined {
    font-size: 18px;
  }

  .spacer {
    flex-grow: 1;
  }

  @media (max-width: 900px) {
    .pill {
      padding: 0 11px;
      font-size: 0;
      gap: 0;
    }

    .pill .material-icons-outlined {
      font-size: 20px;
    }

    .jump {
      font-size: 12px;
      padding: 0 12px;
      gap: 6px;
    }
  }
</style>
