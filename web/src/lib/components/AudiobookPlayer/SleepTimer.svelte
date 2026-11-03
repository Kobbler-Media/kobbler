<script lang="ts">
  import {
    getAudioPlayerState,
    setSleepTimer,
    setInactivityMinutes,
    formatTime,
    type SleepTimerMode
  } from '#lib/stores/audioplayer.svelte.js';

  const player = getAudioPlayerState();

  let showMenu = $state(false);
  let editingMinutes = $state(false);
  let minutesInput = $state(String(player.inactivityMinutes));

  const options: { value: SleepTimerMode; label: string }[] = [
    { value: 'off', label: 'Off' },
    { value: '15', label: '15 minutes' },
    { value: '30', label: '30 minutes' },
    { value: '45', label: '45 minutes' },
    { value: '60', label: '60 minutes' },
    { value: 'end_of_chapter', label: 'End of chapter' },
    { value: 'inactivity', label: 'Auto (no activity)' }
  ];

  function handleSelect(mode: SleepTimerMode) {
    if (mode === 'inactivity') {
      editingMinutes = true;
    } else {
      editingMinutes = false;
    }
    setSleepTimer(mode);
    if (mode === 'off') showMenu = false;
  }

  function handleMinutesChange() {
    const val = parseInt(minutesInput, 10);
    if (!isNaN(val) && val >= 1) {
      setInactivityMinutes(val);
    }
  }

  const isActive = $derived(player.sleepTimerMode !== 'off');
</script>

<div class="sleep-timer">
  <button
    type="button"
    class="timer-btn"
    class:active={isActive}
    onclick={() => (showMenu = !showMenu)}
    title="Sleep timer"
  >
    <span class="material-icons-outlined">bedtime</span>
    {#if isActive && player.sleepTimerRemaining > 0}
      <span class="timer-remaining">{formatTime(player.sleepTimerRemaining)}</span>
    {:else if player.sleepTimerMode === 'end_of_chapter'}
      <span class="timer-remaining">EoC</span>
    {:else if player.sleepTimerMode === 'inactivity'}
      <span class="timer-remaining">{player.inactivityMinutes}m</span>
    {/if}
  </button>

  {#if showMenu}
    <div class="timer-menu">
      {#each options as opt (opt.value)}
        <button
          type="button"
          class="timer-option"
          class:active={player.sleepTimerMode === opt.value}
          onclick={() => handleSelect(opt.value)}
        >
          {opt.label}
        </button>
      {/each}
      {#if editingMinutes || player.sleepTimerMode === 'inactivity'}
        <div class="inactivity-config">
          <label class="inactivity-label">
            <input
              type="number"
              min="1"
              max="120"
              class="inactivity-input"
              bind:value={minutesInput}
              oninput={handleMinutesChange}
            />
            <span>min idle</span>
          </label>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .sleep-timer {
    position: relative;
  }

  .timer-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    padding: 6px 10px;
    border-radius: 8px;
    font-size: 0.8rem;
    opacity: 0.7;
    transition:
      background 0.15s,
      opacity 0.15s;
  }

  .timer-btn .material-icons-outlined {
    font-size: 18px;
  }

  .timer-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    opacity: 1;
  }

  .timer-btn.active {
    opacity: 1;
    color: var(--accent);
  }

  .timer-remaining {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .timer-menu {
    position: absolute;
    bottom: 100%;
    right: 0;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    z-index: 20;
    box-shadow: var(--shadow);
    min-width: 150px;
    margin-bottom: 4px;
  }

  .timer-option {
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    padding: 8px 14px;
    border-radius: 6px;
    font-size: 0.85rem;
    text-align: left;
    white-space: nowrap;
    transition: background 0.15s;
  }

  .timer-option:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .timer-option.active {
    color: var(--accent);
    font-weight: 600;
  }

  .inactivity-config {
    padding: 6px 14px 8px;
    border-top: 1px solid var(--ring);
    margin-top: 2px;
  }

  .inactivity-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8rem;
    color: var(--ink);
    opacity: 0.8;
  }

  .inactivity-input {
    width: 48px;
    padding: 4px 6px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    background: var(--bg);
    color: var(--ink);
    font-size: 0.8rem;
    text-align: center;
  }
</style>
