<script lang="ts">
  import type { StreamPlayback } from '$lib/composables/useStreamPlayback.svelte';
  import StreamOptionsModal from './StreamOptionsModal.svelte';
  import StreamFailurePanel from './StreamFailurePanel.svelte';

  interface Props {
    stream: StreamPlayback;
  }

  let { stream }: Props = $props();
</script>

{#if stream.showStreamModal}
  <StreamOptionsModal
    options={stream.streamOptions}
    loading={stream.streamLoading}
    errors={stream.streamErrors}
    onClose={stream.closeModal}
    onPlay={stream.playOption}
  />
{/if}

{#if stream.failure}
  <StreamFailurePanel
    failure={stream.failure}
    wanting={stream.wanting}
    onClose={stream.dismissFailure}
    onTryAnother={stream.tryAnother}
    onChooseRelease={stream.chooseRelease}
    onWant={stream.want}
  />
{/if}
