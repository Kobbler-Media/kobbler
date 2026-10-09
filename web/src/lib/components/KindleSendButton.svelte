<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api/client';
  import type { KindleTarget, KindleDelivery } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';

  interface Props {
    bookId: string;
  }

  let { bookId }: Props = $props();

  let targets = $state<KindleTarget[]>([]);
  let deliveries = $state<KindleDelivery[]>([]);
  let connected = $state(false);
  let loaded = $state(false);
  let sending = $state<string | null>(null);

  onMount(async () => {
    try {
      const [account, deliveryResult] = await Promise.all([
        api.getKindleAccount(),
        api.getBookKindleDeliveries(bookId)
      ]);
      connected = account.connected;
      targets = account.targets.filter((t) => t.enabled);
      deliveries = deliveryResult.deliveries;
    } catch {
      // Kindle not configured
    }
    loaded = true;
  });

  function isDelivered(targetId: string): KindleDelivery | undefined {
    return deliveries.find((d) => d.target_id === targetId);
  }

  async function handleSend(target: KindleTarget, force: boolean) {
    sending = target.id;
    try {
      await api.sendToKindle(bookId, target.id, force);
      showMessage(`Sending to ${target.device_name || target.device_serial}...`);
      const result = await api.getBookKindleDeliveries(bookId);
      deliveries = result.deliveries;
    } catch {
      showMessage('Failed to queue delivery');
    } finally {
      sending = null;
    }
  }
</script>

{#if loaded && connected && targets.length > 0}
  <div class="section">
    <div class="label">Send to Kindle</div>
    <div class="kindle-targets">
      {#each targets as target (target.id)}
        {@const delivery = isDelivered(target.id)}
        <div class="kindle-target">
          <span class="target-name">{target.device_name || target.device_serial}</span>
          {#if delivery}
            <span class="delivered">Sent</span>
            <button
              class="resend-btn"
              disabled={sending === target.id}
              onclick={() => handleSend(target, true)}
              title="Re-sending creates a duplicate on your Kindle. Delete the existing copy from your device first."
            >
              <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
                <path
                  d="M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z"
                ></path>
              </svg>
            </button>
          {:else}
            <button
              class="btn btn-xs btn-primary"
              disabled={sending === target.id}
              onclick={() => handleSend(target, false)}
            >
              {sending === target.id ? 'Sending...' : 'Send'}
            </button>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .kindle-targets {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .kindle-target {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }
  .target-name {
    flex: 1;
  }
  .delivered {
    color: var(--color-success, #4caf50);
    font-size: 0.8rem;
  }
  .resend-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
  }
  .resend-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }
  .resend-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
