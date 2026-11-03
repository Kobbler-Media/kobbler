<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import {
    getSystemConfig,
    updateSystemConfig,
    type SystemConfig,
    type SystemConfigUpdate
  } from '#lib/api/client.js';

  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);

  const auth = getAuthState();

  onMount(async () => {
    await loadSystemConfig();
  });

  const loadSystemConfig = async () => {
    systemConfigLoading = true;
    systemConfigError = null;
    try {
      systemConfig = await getSystemConfig();
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to load config';
    } finally {
      systemConfigLoading = false;
    }
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        public_url: systemConfig.public_url ?? '',
        activity_limit: systemConfig.activity_limit,
        default_page_size: systemConfig.default_page_size,
        max_page_size: systemConfig.max_page_size,
        watcher_enabled: systemConfig.watcher_enabled,
        watcher_scan_interval_seconds: systemConfig.watcher_scan_interval_seconds,
        media_root_scan_interval_seconds: systemConfig.media_root_scan_interval_seconds,
        video_storage_root: systemConfig.video_storage_root,
        transcode_cache_dir: systemConfig.transcode_cache_dir,
        cache_dir: systemConfig.cache_dir,
        ebook_convert_path: systemConfig.ebook_convert_path,
        ebook_meta_path: systemConfig.ebook_meta_path
      };
      systemConfig = await updateSystemConfig(updates);
      showMessage('Configuration saved');
    } catch (e) {
      systemConfigError = e instanceof Error ? e.message : 'Failed to save config';
    } finally {
      systemConfigSaving = false;
    }
  };
</script>

<svelte:head>
  <title>General Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="general-section">
    <div class="section-header">
      <h2>General Settings</h2>
    </div>

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>Public URL</h3>
          <div data-field="public_url">
            <label>
              Public URL
              <input
                type="text"
                value={systemConfig.public_url ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.public_url = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="https://kobbler.example.com"
              />
              <span class="field-hint">
                The address someone outside the app would use to reach this Kobbler. Kobbler uses it
                whenever it has to produce a link that gets opened elsewhere &mdash; a citation in a
                Google Chat reply, for instance. Include the scheme (<code>https://</code> or
                <code>http://</code>); no trailing slash. Leave empty and those places fall back to
                plain text.
              </span>
            </label>
          </div>
        </div>

        <div class="config-card">
          <h3>Pagination</h3>
          <div data-field="activity_limit">
            <label>
              Activity Limit
              <input type="number" bind:value={systemConfig.activity_limit} min="10" max="1000" />
              <span class="field-hint">Max activity events to display</span>
            </label>
          </div>
          <div class="config-row">
            <div data-field="default_page_size">
              <label>
                Default Page Size
                <input
                  type="number"
                  bind:value={systemConfig.default_page_size}
                  min="10"
                  max="500"
                />
              </label>
            </div>
            <div data-field="max_page_size">
              <label>
                Max Page Size
                <input type="number" bind:value={systemConfig.max_page_size} min="50" max="1000" />
              </label>
            </div>
          </div>
        </div>

        <div class="config-card">
          <h3>File Watcher</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Automatically detect new files added to media roots.
          </p>
          <div data-field="watcher_enabled">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.watcher_enabled} />
              Enable file watcher
            </label>
          </div>
          {#if systemConfig.watcher_enabled}
            <div data-field="watcher_scan_interval_seconds">
              <label>
                Scan Interval (seconds)
                <input
                  type="number"
                  bind:value={systemConfig.watcher_scan_interval_seconds}
                  min="10"
                  max="3600"
                />
                <span class="field-hint"
                  >Fallback polling interval for filesystems without inotify support</span
                >
              </label>
            </div>
          {/if}
          <div data-field="media_root_scan_interval_seconds">
            <label>
              Media Root Scan Interval (seconds)
              <input
                type="number"
                bind:value={systemConfig.media_root_scan_interval_seconds}
                min="60"
                max="86400"
              />
              <span class="field-hint">
                Default interval between filesystem safety-net scans for all media roots. Individual
                media roots can override this. Higher is better for slow NAS; lower only if external
                processes add files outside kobbler's awareness. Default: 1800 (30 minutes).
              </span>
            </label>
          </div>
        </div>

        <div class="config-card">
          <h3>Storage Paths</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Optional directory overrides. Leave empty for defaults.
          </p>
          <div data-field="video_storage_root">
            <label>
              Video Storage Root
              <input
                type="text"
                value={systemConfig.video_storage_root ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.video_storage_root = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="(default: storage_root/video)"
              />
            </label>
          </div>
          <div data-field="transcode_cache_dir">
            <label>
              Transcode Cache Directory
              <input
                type="text"
                value={systemConfig.transcode_cache_dir ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.transcode_cache_dir = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="(default: storage_root/cache/transcode)"
              />
            </label>
          </div>
          <div data-field="cache_dir">
            <label>
              Cache Directory
              <input
                type="text"
                value={systemConfig.cache_dir ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.cache_dir = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="(default: storage_root/cache)"
              />
            </label>
          </div>
        </div>

        <div class="config-card">
          <h3>Conversion Tools</h3>
          <p class="field-hint" style="margin-bottom: 8px;">
            EPUB / MOBI / AZW3 → EPUB and EPUB → KEPUB are handled in-process. ebook-convert is only
            invoked as a fallback for PDF / FB2 / RTF / TXT / HTML / DOCX / LIT / PDB. Leave empty
            to search PATH.
          </p>
          <div data-field="ebook_convert_path">
            <label>
              ebook-convert Path
              <input
                type="text"
                value={systemConfig.ebook_convert_path ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.ebook_convert_path = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="ebook-convert (uses PATH)"
              />
              <span class="field-hint">Calibre tool for format conversion</span>
            </label>
          </div>
          <div data-field="ebook_meta_path">
            <label>
              ebook-meta Path
              <input
                type="text"
                value={systemConfig.ebook_meta_path ?? ''}
                oninput={(e) => {
                  if (systemConfig)
                    systemConfig.ebook_meta_path = (e.target as HTMLInputElement).value || null;
                }}
                placeholder="ebook-meta (uses PATH)"
              />
              <span class="field-hint">Calibre tool for metadata extraction</span>
            </label>
          </div>
        </div>

        <div class="config-actions">
          <button
            class="btn"
            type="button"
            onclick={saveSystemConfig}
            disabled={systemConfigSaving}
          >
            {systemConfigSaving ? 'Saving...' : 'Save Configuration'}
          </button>
          <button
            class="btn alt"
            type="button"
            onclick={loadSystemConfig}
            disabled={systemConfigLoading}
          >
            Reset
          </button>
        </div>
      </div>
    {:else}
      <div class="muted">No configuration loaded</div>
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

  .general-section {
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

  .config-card label:not(.checkbox-label) {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .config-card input[type='text'],
  .config-card input[type='number'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .field-hint code {
    font-size: 10px;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--ring);
  }

  .config-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  .config-actions {
    display: flex;
    gap: 12px;
    padding-top: 8px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
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

  @media (max-width: 768px) {
    .config-row {
      grid-template-columns: 1fr;
    }
  }
</style>
