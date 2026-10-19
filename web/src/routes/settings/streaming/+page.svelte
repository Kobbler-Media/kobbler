<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getSystemConfig,
    updateSystemConfig,
    getStremioKeys,
    createStremioKey,
    deleteStremioKey,
    getStremioAccount,
    linkStremioAccount,
    unlinkStremioAccount,
    syncStremioAccount,
    type SystemConfig,
    type StremioApiKey,
    type StremioAccount
  } from '$lib/api/client';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import { formatTimeAgo } from '$lib/stores/jobs.svelte';

  const auth = getAuthState();

  let systemConfig = $state<SystemConfig | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let savingCache = $state(false);

  let stremioKeys = $state<StremioApiKey[]>([]);
  let loadingKeys = $state(false);
  let creatingKey = $state(false);
  let copiedKeyId = $state<string | null>(null);
  let savingPlayback = $state(false);

  let stremioAccount = $state<StremioAccount | null>(null);
  let stremioEmail = $state('');
  let stremioPassword = $state('');
  let linkingStremio = $state(false);

  const kobloadUrl = $derived(systemConfig?.kobload_base_url ?? null);

  const loadStremioKeys = async () => {
    loadingKeys = true;
    try {
      const resp = await getStremioKeys();
      stremioKeys = resp.keys;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load Stremio keys');
    } finally {
      loadingKeys = false;
    }
  };

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
    await loadStremioKeys();
    if (auth.isAdmin) {
      stremioAccount = (await getStremioAccount().catch(() => ({ account: null }))).account;
    }
  });

  const handleLinkStremio = async () => {
    linkingStremio = true;
    try {
      stremioAccount = (await linkStremioAccount(stremioEmail, stremioPassword)).account;
      stremioPassword = '';
      showMessage('Stremio account linked — syncing Continue Watching');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to link Stremio account');
    } finally {
      linkingStremio = false;
    }
  };

  const handleUnlinkStremio = async () => {
    try {
      await unlinkStremioAccount();
      stremioAccount = null;
      showMessage('Stremio account unlinked');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to unlink Stremio account');
    }
  };

  const handleSyncStremio = async () => {
    try {
      await syncStremioAccount();
      showMessage('Stremio sync queued');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to queue Stremio sync');
    }
  };

  const savePlaybackDefaults = async () => {
    if (!systemConfig) return;
    savingPlayback = true;
    try {
      systemConfig = await updateSystemConfig({
        preferred_audio_language: systemConfig.preferred_audio_language,
        preferred_subtitle_language: systemConfig.preferred_subtitle_language,
        mkv_cue_trim_enabled: systemConfig.mkv_cue_trim_enabled
      });
      showMessage('Playback defaults saved');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save playback defaults');
    } finally {
      savingPlayback = false;
    }
  };

  const saveCacheSettings = async () => {
    if (!systemConfig) return;
    savingCache = true;
    try {
      systemConfig = await updateSystemConfig({
        debrid_cache_dir: systemConfig.debrid_cache_dir ?? undefined,
        debrid_cache_max_bytes: systemConfig.debrid_cache_max_bytes ?? undefined,
        debrid_cache_ttl_secs: systemConfig.debrid_cache_ttl_secs ?? undefined
      });
      showMessage('Cache settings saved');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save cache settings');
    } finally {
      savingCache = false;
    }
  };

  const handleGenerateKey = async () => {
    creatingKey = true;
    try {
      const key = await createStremioKey('Stremio Key');
      stremioKeys = [key, ...stremioKeys];
      showMessage('Stremio API key created');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create key');
    } finally {
      creatingKey = false;
    }
  };

  const handleDeleteKey = async (id: string) => {
    try {
      await deleteStremioKey(id);
      stremioKeys = stremioKeys.filter((k) => k.id !== id);
      showMessage('Stremio API key revoked');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to revoke key');
    }
  };

  const copyKeyUrl = async (apiKey: string, keyId: string) => {
    const url = `${window.location.origin}/stremio/${apiKey}/manifest.json`;
    try {
      await navigator.clipboard.writeText(url);
      copiedKeyId = keyId;
      showMessage('Addon URL copied to clipboard');
      setTimeout(() => (copiedKeyId = null), 2000);
    } catch {
      showMessage('Failed to copy to clipboard');
    }
  };
</script>

<svelte:head>
  <title>Streaming Settings — Kobbler</title>
</svelte:head>

{#if !auth.canManageContent}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="streaming-section">
    <div class="section-header">
      <h2>Streaming</h2>
    </div>

    {#if loading}
      <div class="loading-card">Loading configuration...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
      <div class="config-form">
        <div class="config-card">
          <h3>Stremio Addon</h3>
          <p class="card-description muted">
            Use Kobbler as a Stremio addon for streaming on mobile and TV devices. All streams are
            proxied through Kobbler for privacy.
          </p>

          <div class="field">
            <span class="field-label">API Keys</span>

            {#if loadingKeys}
              <span class="muted" style="font-size: 13px;">Loading keys...</span>
            {:else if stremioKeys.length === 0}
              <span class="muted" style="font-size: 13px;"
                >No API keys yet. Generate one to get started.</span
              >
            {:else}
              <div class="key-list">
                {#each stremioKeys as key (key.id)}
                  <div class="key-row">
                    <div class="key-info">
                      <span class="key-name">{key.name}</span>
                      <code class="key-value">{key.api_key}</code>
                    </div>
                    <div class="key-actions">
                      <button
                        class="btn btn-icon"
                        type="button"
                        onclick={() => copyKeyUrl(key.api_key, key.id)}
                        title="Copy addon URL"
                      >
                        <svg
                          viewBox="0 0 24 24"
                          width="16"
                          height="16"
                          style="min-width:16px;min-height:16px"
                          fill="currentColor"
                        >
                          {#if copiedKeyId === key.id}
                            <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"></path>
                          {:else}
                            <path
                              d="M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z"
                            ></path>
                          {/if}
                        </svg>
                      </button>
                      <button
                        class="btn btn-icon btn-danger"
                        type="button"
                        onclick={() => handleDeleteKey(key.id)}
                        title="Revoke key"
                      >
                        <svg
                          viewBox="0 0 24 24"
                          width="16"
                          height="16"
                          style="min-width:16px;min-height:16px"
                          fill="currentColor"
                        >
                          <path
                            d="M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z"
                          ></path>
                        </svg>
                      </button>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}

            <button
              class="btn btn-primary"
              type="button"
              onclick={handleGenerateKey}
              disabled={creatingKey}
            >
              {creatingKey ? 'Generating...' : 'Generate Key'}
            </button>

            <span class="field-hint">
              Each key generates a unique addon URL. Add the URL in Stremio under Settings &rarr;
              Addons.
            </span>
          </div>
        </div>

        {#if auth.isAdmin}
          <div class="config-card">
            <h3>Stremio Continue Watching</h3>
            <p class="card-description muted">
              Link a Stremio account to show its Continue Watching as a row on every profile's home
              page, below Recently Added. Kids profiles only see titles their rating rules allow.
            </p>

            {#if stremioAccount}
              <div class="field">
                <span class="field-label">Linked account</span>
                <span>{stremioAccount.email}</span>
                <span class="field-hint">
                  <MetaLine
                    items={[
                      `${stremioAccount.item_count} titles`,
                      stremioAccount.last_synced_at
                        ? `synced ${formatTimeAgo(stremioAccount.last_synced_at)}`
                        : 'not synced yet'
                    ]}
                  />
                </span>
                {#if stremioAccount.last_error}
                  <div class="warning-box">{stremioAccount.last_error}</div>
                {/if}
              </div>
              <div class="key-actions">
                <button class="btn btn-primary" type="button" onclick={handleSyncStremio}>
                  Sync now
                </button>
                <button class="btn btn-danger" type="button" onclick={handleUnlinkStremio}>
                  Unlink
                </button>
              </div>
            {:else}
              <form
                onsubmit={(e) => {
                  e.preventDefault();
                  handleLinkStremio();
                }}
              >
                <div class="form-group">
                  <label for="stremio-email">Stremio email</label>
                  <input
                    id="stremio-email"
                    type="email"
                    class="text-input"
                    autocomplete="off"
                    bind:value={stremioEmail}
                  />
                </div>
                <div class="form-group">
                  <label for="stremio-password">Password</label>
                  <input
                    id="stremio-password"
                    type="password"
                    class="text-input"
                    autocomplete="off"
                    bind:value={stremioPassword}
                  />
                  <span class="form-hint">
                    Used once to sign in. Only Stremio's session key is stored.
                  </span>
                </div>
                <button
                  class="btn btn-primary"
                  type="submit"
                  disabled={linkingStremio || !stremioEmail || !stremioPassword}
                >
                  {linkingStremio ? 'Linking...' : 'Link account'}
                </button>
              </form>
            {/if}
          </div>
        {/if}

        <div class="config-card">
          <h3>Debrid Providers</h3>
          <p class="card-description muted">
            Debrid providers (Real-Debrid, AllDebrid, Premiumize, TorBox) are configured in Kobload.
            Kobbler proxies debrid streams for privacy and adds timeline thumbnails, skip-intro
            detection, and instant replay.
          </p>

          {#if kobloadUrl}
            <a class="link-button" href={kobloadUrl} target="_blank" rel="noopener noreferrer">
              Open Kobload Settings
              <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
                <path
                  d="M19 19H5V5h7V3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2v-7h-2v7zM14 3v2h3.59l-9.83 9.83 1.41 1.41L19 6.41V10h2V3h-7z"
                ></path>
              </svg>
            </a>
          {:else}
            <div class="warning-box">
              Configure a <strong>Kobload connection</strong> in
              <a href="/settings/obtain">Obtain Settings</a> first.
            </div>
          {/if}
        </div>

        <div class="config-card">
          <h3>Stream Cache</h3>
          <p class="card-description muted">
            Debrid streams are cached locally during playback for seeking, thumbnails, and skip
            detection. Expired cache files are cleaned up automatically.
          </p>

          {#if systemConfig}
            <div class="form-group" data-field="debrid_cache_dir">
              <label for="cache-dir">Cache directory</label>
              <input
                id="cache-dir"
                type="text"
                class="text-input"
                placeholder="Default: storage_root/debrid_cache"
                bind:value={systemConfig.debrid_cache_dir}
              />
              <span class="form-hint">Leave blank for default. Absolute path on the server.</span>
            </div>

            <div class="form-group" data-field="debrid_cache_max_bytes">
              <label for="cache-max-gb">Maximum cache size (GB)</label>
              <input
                id="cache-max-gb"
                type="number"
                class="text-input"
                min="1"
                step="1"
                value={Math.round(
                  (systemConfig.debrid_cache_max_bytes ?? 53687091200) / 1073741824
                )}
                onchange={(e) => {
                  const gb = parseInt(e.currentTarget.value) || 50;
                  if (systemConfig) systemConfig.debrid_cache_max_bytes = gb * 1073741824;
                }}
              />
              <span class="form-hint"
                >Oldest cache files are evicted when this limit is reached.</span
              >
            </div>

            <div class="form-group" data-field="debrid_cache_ttl_secs">
              <label for="cache-ttl-days">Cache TTL (days)</label>
              <input
                id="cache-ttl-days"
                type="number"
                class="text-input"
                min="1"
                step="1"
                value={Math.round((systemConfig.debrid_cache_ttl_secs ?? 604800) / 86400)}
                onchange={(e) => {
                  const days = parseInt(e.currentTarget.value) || 7;
                  if (systemConfig) systemConfig.debrid_cache_ttl_secs = days * 86400;
                }}
              />
              <span class="form-hint">Cache files older than this are eligible for cleanup.</span>
            </div>

            <button class="save-button" onclick={saveCacheSettings} disabled={savingCache}>
              {savingCache ? 'Saving...' : 'Save Cache Settings'}
            </button>
          {/if}
        </div>

        <div class="config-card">
          <h3>Playback Defaults</h3>
          <p class="card-description muted">
            Preferred languages for audio and subtitles. When starting playback, the matching audio
            track is selected automatically. If no matching audio is found, subtitles in the
            preferred language are enabled instead.
          </p>

          {#if systemConfig}
            <div class="form-group" data-field="preferred_audio_language">
              <label for="pref-audio-lang">Preferred audio language</label>
              <input
                id="pref-audio-lang"
                type="text"
                class="text-input"
                placeholder="e.g. english, french, japanese"
                bind:value={systemConfig.preferred_audio_language}
              />
              <span class="form-hint">Matches ISO codes (en, eng) and full names (english).</span>
            </div>

            <div class="form-group" data-field="preferred_subtitle_language">
              <label for="pref-sub-lang">Preferred subtitle language</label>
              <input
                id="pref-sub-lang"
                type="text"
                class="text-input"
                placeholder="e.g. english, french, japanese"
                bind:value={systemConfig.preferred_subtitle_language}
              />
              <span class="form-hint">Used as fallback when no matching audio track exists.</span>
            </div>

            <div class="form-group" data-field="mkv_cue_trim_enabled">
              <label class="checkbox-label">
                <input type="checkbox" bind:checked={systemConfig.mkv_cue_trim_enabled} />
                Trim oversized Matroska cue indexes
              </label>
              <span class="form-hint">
                Releases carrying dozens of subtitle languages index every subtitle line, which
                players parse in full before the first frame. Serves direct-play byte ranges with
                the unused parts of that index removed. The files on disk are not modified.
              </span>
            </div>

            <button class="save-button" onclick={savePlaybackDefaults} disabled={savingPlayback}>
              {savingPlayback ? 'Saving...' : 'Save Playback Defaults'}
            </button>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }

  .streaming-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .config-form {
    display: grid;
    gap: 16px;
  }

  .config-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
    display: grid;
    gap: 12px;
  }

  .config-card h3 {
    margin: 0 0 4px;
    font-size: 15px;
    font-weight: 600;
  }

  .card-description {
    font-size: 13px;
    margin: 0 0 8px;
  }

  .field {
    display: grid;
    gap: 6px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .btn-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--surface, var(--card));
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
  }

  .btn-icon:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .warning-box {
    font-size: 13px;
    padding: 10px 14px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--ink);
  }

  .warning-box a {
    color: var(--accent);
    text-decoration: underline;
  }

  .link-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 12px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 13px;
    font-weight: 600;
    text-decoration: none;
    width: fit-content;
    cursor: pointer;
    transition: background 0.15s;
  }

  .link-button:hover {
    background: var(--ring);
  }

  .key-list {
    display: grid;
    gap: 8px;
  }

  .key-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--page);
  }

  .key-info {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .key-name {
    font-size: 12px;
    color: var(--muted);
  }

  .key-value {
    font-size: 12px;
    font-family: monospace;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .key-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .btn-primary {
    padding: 8px 16px;
    border-radius: 10px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: white;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    width: fit-content;
    transition: opacity 0.15s;
  }

  .btn-primary:hover {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger {
    color: var(--danger, #e53e3e);
  }

  .btn-danger:hover {
    background: color-mix(in srgb, var(--danger, #e53e3e) 12%, transparent);
  }

  .form-group {
    display: grid;
    gap: 4px;
    margin-bottom: 12px;
  }

  .form-group label {
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }

  .text-input {
    padding: 6px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--field);
    color: var(--ink);
    font-size: 13px;
    width: 100%;
    box-sizing: border-box;
  }

  .text-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .form-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }

  .save-button {
    margin-top: 8px;
    padding: 6px 16px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: var(--accent);
    color: white;
    font-size: 13px;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .save-button:hover {
    opacity: 0.9;
  }

  .save-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }
</style>
