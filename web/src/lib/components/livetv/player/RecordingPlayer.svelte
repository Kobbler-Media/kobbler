<script lang="ts">
  import { onMount } from 'svelte';
  import VideoPlayer from '#lib/components/VideoPlayer/VideoPlayer.svelte';
  import AdBreakPanel from '../AdBreakPanel.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { detectCapabilities } from '#lib/utils/media.js';
  import * as api from '#lib/api/client.js';
  import type {
    LivetvAdBreak,
    LivetvRecording,
    PlaybackSessionResponse,
    SkipSegment
  } from '#lib/types.js';

  interface Props {
    recording: LivetvRecording;
    onClose: () => void;
  }

  let { recording, onClose }: Props = $props();

  // The posture rides on the recording already resolved: the settings endpoint it
  // used to come from is admin-only, so a contributor never saw the owner's policy.
  const posture = $derived(recording.ad_posture);
  // Ad skip collapses on news and live sport, and the server says which those are.
  const autoSkip = $derived(posture === 'auto' && !recording.ad_skip_disabled);

  let session = $state<PlaybackSessionResponse | null>(null);
  let breaks = $state<LivetvAdBreak[]>([]);
  let adSegments: SkipSegment[] = [];
  let capabilities = detectCapabilities();
  let player = $state<ReturnType<typeof VideoPlayer> | undefined>();

  function toSegment(b: LivetvAdBreak): SkipSegment {
    return {
      segment_type: 'ad_break',
      start_seconds: b.start_seconds,
      end_seconds: b.end_seconds,
      confidence: b.confidence ?? undefined,
      source: b.method
    };
  }

  onMount(async () => {
    let playback: PlaybackSessionResponse;
    try {
      playback = await api.getLivetvRecordingPlayback(recording.id);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to open the recording');
      onClose();
      return;
    }

    // Ad breaks live on their own endpoint, so shading and the Skip Ads button
    // still work when detection has not run yet — the list is simply empty.
    try {
      const result = await api.getLivetvAdBreaks(recording.id);
      breaks = result.items ?? [];
    } catch {
      breaks = recording.ad_breaks ?? [];
    }

    adSegments = breaks.map(toSegment);
    playback.skip_segments = [...(playback.skip_segments ?? []), ...adSegments];
    session = playback;
  });

  async function saveSegment(seg: SkipSegment, startSeconds: number, endSeconds: number) {
    const index = adSegments.indexOf(seg);
    if (index < 0) return;
    const updated = await api.updateLivetvAdBreak(recording.id, breaks[index].id, {
      start_seconds: startSeconds,
      end_seconds: endSeconds
    });
    breaks = breaks.map((b, i) => (i === index ? updated : b));
    showMessage('Ad break corrected — the fingerprint now covers every future airing');
  }

  async function deleteSegment(seg: SkipSegment) {
    const index = adSegments.indexOf(seg);
    if (index < 0) return;
    await api.deleteLivetvAdBreak(recording.id, breaks[index].id);
    seg.start_seconds = 0;
    seg.end_seconds = 0;
    breaks = breaks.filter((_, i) => i !== index);
    showMessage('Ad break removed');
  }

  function adjust(target: LivetvAdBreak) {
    const index = breaks.findIndex((b) => b.id === target.id);
    if (index < 0) return;
    player?.openSegmentEditor(adSegments[index]);
  }

  const subtitle = $derived(
    [recording.episode_number, recording.provenance?.aired ?? recording.channel_name]
      .filter(Boolean)
      .join(' · ')
  );
</script>

{#if session}
  <VideoPlayer
    bind:this={player}
    sessionResponse={session}
    creationParams={{ media_type: 'recording', file_id: recording.id, capabilities }}
    mediaInfo={{ title: recording.title, subtitle }}
    skipButtonTypes={posture === 'shade'
      ? ['intro', 'credits', 'recap', 'post_credits_scene']
      : undefined}
    autoSkipTypes={autoSkip ? ['ad_break'] : undefined}
    onSaveSkipSegment={saveSegment}
    onDeleteSkipSegment={deleteSegment}
    {onClose}
  >
    {#snippet bottomPanel()}
      <AdBreakPanel {breaks} {posture} onAdjust={adjust} />
    {/snippet}
  </VideoPlayer>
{/if}
