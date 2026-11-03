<script lang="ts">
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';

  interface Props {
    channelId: string;
    title?: string;
    startTime?: string;
    endTime?: string;
    epgId?: string;
  }

  let { channelId, title = 'Manual Recording', startTime, endTime, epgId }: Props = $props();

  let recording = $state(false);

  async function handleRecord() {
    recording = true;
    try {
      if (epgId) {
        await api.recordFromEpg({ epg_id: epgId });
      } else {
        const now = new Date();
        const recStart = startTime ?? now.toISOString();
        const recEnd = endTime ?? new Date(now.getTime() + 3600000).toISOString();

        await api.createIptvRecording({
          channel_id: channelId,
          title,
          start_time: recStart,
          end_time: recEnd
        });
      }
      showMessage('Recording scheduled');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create recording');
    } finally {
      recording = false;
    }
  }
</script>

<button class="record-btn" type="button" disabled={recording} onclick={handleRecord} title="Record">
  <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
    <circle cx="12" cy="12" r="8"></circle>
  </svg>
  <span>{recording ? 'Scheduling...' : 'Record'}</span>
</button>

<style>
  .record-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border-radius: 8px;
    border: 1px solid rgba(239, 68, 68, 0.3);
    background: rgba(239, 68, 68, 0.1);
    color: #ef4444;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .record-btn:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.2);
    border-color: rgba(239, 68, 68, 0.5);
  }

  .record-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
