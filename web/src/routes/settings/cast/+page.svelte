<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { getSystemConfig, updateSystemConfig, type SystemConfig } from '$lib/api/client';

  const auth = getAuthState();

  let systemConfig = $state<SystemConfig | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);

  onMount(async () => {
    loading = true;
    error = null;
    try {
      systemConfig = await getSystemConfig();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load config';
    } finally {
      loading = false;
    }
  });

  const save = async () => {
    if (!systemConfig) return;
    saving = true;
    try {
      systemConfig = await updateSystemConfig({
        cast_receiver_app_id: systemConfig.cast_receiver_app_id ?? undefined
      });
      showMessage('Cast settings saved');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save');
    } finally {
      saving = false;
    }
  };

  const receiverUrl = $derived(
    typeof window !== 'undefined' ? `${window.location.origin}/cast-receiver/receiver.html` : ''
  );
</script>

<svelte:head>
  <title>Cast Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="cast-section">
    <div class="section-header">
      <h2>Google Cast</h2>
    </div>

    {#if loading}
      <div class="loading-card">Loading configuration...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>Receiver Setup</h3>
          <p class="card-description muted">
            Cast lets you stream from Kobbler to Chromecast, Google TV, and other Cast-enabled
            devices. Requires a one-time registration on the Google Cast Developer Console.
          </p>

          <div class="setup-steps">
            <div class="step">
              <span class="step-num">1</span>
              <div class="step-content">
                <span class="step-title">Register on the Cast Developer Console</span>
                <p class="step-description">
                  <a
                    href="https://cast.google.com/publish/"
                    target="_blank"
                    rel="noopener noreferrer">cast.google.com/publish</a
                  > — one-time $5 fee. Create a new "Custom Receiver" application.
                </p>
              </div>
            </div>
            <div class="step">
              <span class="step-num">2</span>
              <div class="step-content">
                <span class="step-title">Set the receiver URL</span>
                <p class="step-description">Point your custom receiver to:</p>
                <code class="receiver-url">{receiverUrl}</code>
              </div>
            </div>
            <div class="step">
              <span class="step-num">3</span>
              <div class="step-content">
                <span class="step-title">Paste the App ID below</span>
                <p class="step-description">
                  The console gives you an App ID (e.g. <code>A1B2C3D4</code>). Enter it here to
                  enable the Cast button in the player.
                </p>
              </div>
            </div>
          </div>

          <div class="field" data-field="cast_receiver_app_id">
            <label for="cast-app-id" class="field-label">Cast Receiver App ID</label>
            <input
              id="cast-app-id"
              type="text"
              class="field-input"
              placeholder="e.g. A1B2C3D4"
              bind:value={systemConfig.cast_receiver_app_id}
            />
          </div>

          <div class="form-actions">
            <button class="btn btn-primary" type="button" onclick={save} disabled={saving}>
              {saving ? 'Saving...' : 'Save'}
            </button>
          </div>
        </div>

        <div class="config-card">
          <h3>Device Support</h3>
          <p class="card-description muted">
            When registering your receiver, check both <strong>Video</strong> and
            <strong>Audio</strong> device types. This covers Chromecasts, Google TV, Nest displays, and
            smart speakers.
          </p>
          <div class="device-list">
            <div class="device-item">
              <span class="material-icons-outlined">tv</span>
              <div>
                <strong>Chromecast / Google TV</strong>
                <span class="muted">Full video + audio, skip segments, thumbnails</span>
              </div>
            </div>
            <div class="device-item">
              <span class="material-icons-outlined">speaker</span>
              <div>
                <strong>Nest / Google Home speakers</strong>
                <span class="muted">Audio casting (audiobook support planned)</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .cast-section {
    max-width: 720px;
  }

  .section-header h2 {
    margin: 0 0 20px;
    font-size: 22px;
    font-weight: 600;
  }

  .config-form {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .config-card {
    background: var(--surface-1, #1a1a2e);
    border: 1px solid var(--border, rgba(255, 255, 255, 0.08));
    border-radius: 10px;
    padding: 20px;
  }

  .config-card h3 {
    margin: 0 0 4px;
    font-size: 16px;
    font-weight: 600;
  }

  .card-description {
    margin: 0 0 16px;
    font-size: 13px;
    line-height: 1.5;
  }

  .muted {
    color: var(--text-muted, rgba(255, 255, 255, 0.5));
  }

  .setup-steps {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 20px;
  }

  .step {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .step-num {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    min-width: 24px;
    border-radius: 50%;
    background: var(--accent, #7c5cfc);
    color: #fff;
    font-size: 12px;
    font-weight: 600;
    margin-top: 1px;
  }

  .step-content {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .step-title {
    font-size: 14px;
    font-weight: 500;
  }

  .step-description {
    font-size: 13px;
    color: var(--text-muted, rgba(255, 255, 255, 0.5));
    margin: 0;
    line-height: 1.4;
  }

  .step-description a {
    color: var(--accent, #7c5cfc);
    text-decoration: none;
  }

  .step-description a:hover {
    text-decoration: underline;
  }

  .receiver-url {
    display: block;
    margin-top: 4px;
    padding: 6px 10px;
    background: rgba(0, 0, 0, 0.3);
    border-radius: 4px;
    font-size: 12px;
    word-break: break-all;
    user-select: all;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 16px;
  }

  .field-label {
    font-size: 13px;
    font-weight: 500;
  }

  .field-input {
    padding: 8px 12px;
    background: var(--surface-0, #111);
    border: 1px solid var(--border, rgba(255, 255, 255, 0.12));
    border-radius: 6px;
    color: inherit;
    font-size: 14px;
    font-family: monospace;
    max-width: 280px;
  }

  .field-input::placeholder {
    color: var(--text-muted, rgba(255, 255, 255, 0.3));
  }

  .form-actions {
    display: flex;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--accent, #7c5cfc);
    color: #fff;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .device-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .device-item {
    display: flex;
    gap: 12px;
    align-items: center;
  }

  .device-item .material-icons-outlined {
    font-size: 24px;
    color: var(--text-muted, rgba(255, 255, 255, 0.4));
  }

  .device-item div {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .device-item strong {
    font-size: 13px;
    font-weight: 500;
  }

  .device-item .muted {
    font-size: 12px;
  }

  .loading-card {
    padding: 40px;
    text-align: center;
    color: var(--text-muted, rgba(255, 255, 255, 0.5));
  }

  .error {
    padding: 12px 16px;
    background: rgba(255, 80, 80, 0.1);
    border: 1px solid rgba(255, 80, 80, 0.2);
    border-radius: 8px;
    color: #ff5050;
    font-size: 13px;
  }

  .permission-denied {
    padding: 40px;
    text-align: center;
  }
</style>
