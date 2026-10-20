<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import type { StreamFailurePanel } from '$lib/composables/useStreamPlayback.svelte';

  interface Props {
    failure: StreamFailurePanel;
    wanting: boolean;
    onClose: () => void;
    onTryAnother: () => void;
    onChooseRelease: () => void;
    onWant: () => void;
  }

  let { failure, wanting, onClose, onTryAnother, onChooseRelease, onWant }: Props = $props();
</script>

<Modal {onClose} title={failure.title} width="460px">
  <div class="failure-body">
    <p class="summary">{failure.summary}</p>
    {#if failure.details.length > 0}
      <details class="details">
        <summary>Details</summary>
        <ul>
          {#each failure.details as line, i (i)}
            <li>{line}</li>
          {/each}
        </ul>
      </details>
    {/if}
  </div>
  {#snippet footer()}
    {#if failure.canWant}
      <button class="btn alt" type="button" onclick={onWant} disabled={wanting}>
        {wanting ? 'Wanting…' : 'Want'}
      </button>
    {/if}
    {#if failure.canChoose}
      <button class="btn alt" type="button" onclick={onChooseRelease}>Choose a release</button>
    {/if}
    {#if failure.canTryAnother}
      <button class="btn" type="button" onclick={onTryAnother}>Try another</button>
    {:else}
      <button class="btn" type="button" onclick={onClose}>Close</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .failure-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .summary {
    margin: 0;
    font-size: 14px;
    line-height: 1.5;
    color: var(--ink);
  }

  .details {
    font-size: 12px;
    color: var(--muted);
  }

  .details summary {
    cursor: pointer;
  }

  .details ul {
    margin: 8px 0 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    word-break: break-word;
  }
</style>
