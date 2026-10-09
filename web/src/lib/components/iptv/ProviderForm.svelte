<script lang="ts">
  import type { IptvProvider } from '$lib/types';

  interface Props {
    onsubmit: (data: {
      name: string;
      url: string;
      username: string;
      password: string;
      preferred_format: string;
      sync_interval_secs: number;
      enabled?: boolean;
    }) => void;
    oncancel: () => void;
    saving?: boolean;
    provider?: IptvProvider;
  }

  import { untrack } from 'svelte';

  let { onsubmit, oncancel, saving = false, provider }: Props = $props();

  const editing = $derived(!!provider);

  // One-time capture of provider values for form state — intentionally not reactive
  let name = $state(untrack(() => provider?.name ?? ''));
  let url = $state(untrack(() => provider?.url ?? ''));
  let username = $state(untrack(() => provider?.username ?? ''));
  let password = $state('');
  let preferredFormat = $state(untrack(() => provider?.preferred_format ?? 'ts'));
  let syncInterval = $state(
    untrack(() => (provider ? Math.round(provider.sync_interval_secs / 3600) : 24))
  );
  let enabled = $state(untrack(() => provider?.enabled ?? true));

  function handleSubmit(e: Event) {
    e.preventDefault();
    const data: Parameters<Props['onsubmit']>[0] = {
      name,
      url,
      username,
      password,
      preferred_format: preferredFormat,
      sync_interval_secs: syncInterval * 3600
    };
    if (editing) {
      data.enabled = enabled;
    }
    onsubmit(data);
  }
</script>

<form class="provider-form" onsubmit={handleSubmit}>
  <div class="form-field">
    <label for="pf-name">Name</label>
    <input id="pf-name" type="text" bind:value={name} placeholder="My IPTV Provider" required />
  </div>

  <div class="form-field">
    <label for="pf-url">Server URL</label>
    <input
      id="pf-url"
      type="url"
      bind:value={url}
      placeholder="http://provider.example.com"
      required
    />
  </div>

  <div class="form-row">
    <div class="form-field">
      <label for="pf-user">Username</label>
      <input id="pf-user" type="text" bind:value={username} required />
    </div>
    <div class="form-field">
      <label for="pf-pass">Password</label>
      <input
        id="pf-pass"
        type="password"
        bind:value={password}
        placeholder={editing ? '(unchanged)' : ''}
        required={!editing}
      />
    </div>
  </div>

  <div class="form-row">
    <div class="form-field">
      <label for="pf-format">Preferred Format</label>
      <select id="pf-format" bind:value={preferredFormat}>
        <option value="ts">MPEG-TS (.ts)</option>
        <option value="m3u8">HLS (.m3u8)</option>
      </select>
    </div>
    <div class="form-field">
      <label for="pf-interval">Sync Interval (hours)</label>
      <input id="pf-interval" type="number" bind:value={syncInterval} min="1" max="168" />
    </div>
  </div>

  {#if editing}
    <div class="form-field toggle-field">
      <label for="pf-enabled">Enabled</label>
      <button
        id="pf-enabled"
        type="button"
        class="toggle"
        class:active={enabled}
        onclick={() => (enabled = !enabled)}
        aria-pressed={enabled}
        aria-label="Toggle provider enabled"
      >
        <span class="toggle-thumb"></span>
      </button>
    </div>
  {/if}

  <div class="form-actions">
    <button type="button" class="btn-secondary" onclick={oncancel}>Cancel</button>
    <button
      type="submit"
      class="btn-primary"
      disabled={saving || !name || !url || !username || (!editing && !password)}
    >
      {#if saving}
        {editing ? 'Saving...' : 'Adding...'}
      {:else}
        {editing ? 'Save Changes' : 'Add Provider'}
      {/if}
    </button>
  </div>
</form>

<style>
  .provider-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }

  .form-field label {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .form-field input,
  .form-field select {
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
    transition: border-color 0.15s;
  }

  .form-field input:focus,
  .form-field select:focus {
    border-color: var(--accent);
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  @media (max-width: 480px) {
    .form-row {
      flex-direction: column;
    }
  }

  .toggle-field {
    flex-direction: row;
    align-items: center;
    gap: 12px;
  }

  .toggle {
    position: relative;
    width: 40px;
    height: 22px;
    border-radius: 11px;
    border: 1px solid var(--ring);
    background: var(--card);
    cursor: pointer;
    padding: 0;
    transition: background 0.2s;
  }

  .toggle.active {
    background: var(--accent);
    border-color: var(--accent);
  }

  .toggle-thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--ink);
    transition: transform 0.2s;
  }

  .toggle.active .toggle-thumb {
    transform: translateX(18px);
    background: var(--bg);
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  .btn-primary,
  .btn-secondary {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--ring);
    transition: all 0.15s;
  }

  .btn-primary {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--card);
    color: var(--ink);
  }

  .btn-secondary:hover {
    background: var(--ring);
  }
</style>
