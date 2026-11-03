<script lang="ts">
  import { showMessage } from '#lib/context/app.svelte.js';
  import {
    getDevSnapshotStatus,
    saveDevSnapshot,
    deleteDevSnapshot,
    type DevSnapshotStatus
  } from '#lib/api/client.js';
  import { onMount } from 'svelte';

  let status: DevSnapshotStatus | null = $state(null);
  let loading = $state(true);
  let saving = $state(false);
  let deleting = $state(false);

  onMount(async () => {
    await loadStatus();
  });

  async function loadStatus() {
    loading = true;
    try {
      status = await getDevSnapshotStatus();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load dev snapshot status');
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    saving = true;
    try {
      const result = await saveDevSnapshot();
      showMessage(`Snapshot saved: ${result.books_count} books, ${result.files_count} files`);
      await loadStatus();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to save dev snapshot');
    } finally {
      saving = false;
    }
  }

  async function handleDelete() {
    const confirmed = confirm('Delete the dev snapshot? This cannot be undone.');
    if (!confirmed) return;

    deleting = true;
    try {
      await deleteDevSnapshot();
      showMessage('Dev snapshot deleted');
      await loadStatus();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete dev snapshot');
    } finally {
      deleting = false;
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }

  function formatDate(dateStr: string): string {
    return new Date(dateStr).toLocaleString();
  }
</script>

<svelte:head>
  <title>Dev Tools — Kobbler</title>
</svelte:head>

<div class="page-container">
  <div class="dev-tools">
    <div class="section-header">
      <h2>Dev Tools</h2>
      <span class="dev-badge">DEV MODE</span>
    </div>

    {#if !status?.dev_mode && !loading}
      <div class="warning-card">
        <p>Dev mode is not enabled. Set <code>DEV_MODE=1</code> in your environment.</p>
      </div>
    {:else}
      <div class="snapshot-card">
        <h3>Dev Snapshot</h3>
        <p class="muted">
          Save your current library state as the dev default. When <code>DEV_AUTO_RESTORE=1</code>,
          this snapshot will be automatically restored on startup.
        </p>

        {#if loading}
          <div class="loading">Loading...</div>
        {:else if status}
          <div class="snapshot-status">
            {#if status.exists}
              <div class="status-row">
                <span class="label">Status:</span>
                <span class="value success">Snapshot exists</span>
              </div>
              {#if status.size}
                <div class="status-row">
                  <span class="label">Size:</span>
                  <span class="value">{formatBytes(status.size)}</span>
                </div>
              {/if}
              {#if status.created_at}
                <div class="status-row">
                  <span class="label">Created:</span>
                  <span class="value">{formatDate(status.created_at)}</span>
                </div>
              {/if}
            {:else}
              <div class="status-row">
                <span class="label">Status:</span>
                <span class="value muted">No snapshot saved</span>
              </div>
            {/if}
          </div>

          <div class="actions">
            <button class="btn btn-primary" onclick={handleSave} disabled={saving || deleting}>
              {saving ? 'Saving...' : 'Save as Dev Default'}
            </button>
            {#if status.exists}
              <button class="btn btn-danger" onclick={handleDelete} disabled={saving || deleting}>
                {deleting ? 'Deleting...' : 'Clear Dev Default'}
              </button>
            {/if}
          </div>
        {/if}
      </div>

      <div class="info-card">
        <h3>How it works</h3>
        <ul>
          <li>
            <strong>Save as Dev Default</strong> creates a backup at
            <code>config/dev-snapshot.tar.gz</code>
          </li>
          <li>
            When <code>DEV_AUTO_RESTORE=1</code>, the snapshot is restored on container startup
          </li>
          <li>This gives you a consistent starting point for development</li>
          <li>The snapshot includes all books, metadata, users, devices, and settings</li>
        </ul>
      </div>

      <div class="info-card">
        <h3>Dev Environment</h3>
        <ul>
          <li>
            <strong>Fixed credentials:</strong> Set <code>DEV_ADMIN_PASSWORD</code> in
            <code>.env.dev</code> to avoid extracting passwords from logs
          </li>
          <li>
            <strong>Fixed ingest token:</strong> Set <code>DEV_INGEST_TOKEN</code> for consistent Kobload
            integration
          </li>
          <li>
            <strong>Cross-project networking:</strong> Kobbler and Kobload share the
            <code>kob_dev</code> Docker network
          </li>
        </ul>
      </div>
    {/if}
  </div>
</div>

<style>
  .dev-tools {
    display: grid;
    gap: 16px;
    max-width: 600px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .dev-badge {
    background: var(--accent);
    color: white;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.5px;
  }

  .snapshot-card,
  .info-card,
  .warning-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 20px;
  }

  .warning-card {
    border-color: var(--danger);
    background: rgba(239, 68, 68, 0.1);
  }

  .warning-card p {
    margin: 0;
    color: var(--danger);
  }

  .snapshot-card h3,
  .info-card h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .snapshot-card p {
    margin: 0 0 16px;
    font-size: 13px;
  }

  .snapshot-status {
    display: grid;
    gap: 8px;
    margin-bottom: 16px;
    padding: 12px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 8px;
  }

  :global([data-theme='light']) .snapshot-status {
    background: rgba(0, 0, 0, 0.03);
  }

  .status-row {
    display: flex;
    gap: 8px;
    font-size: 13px;
  }

  .status-row .label {
    color: var(--muted);
    min-width: 80px;
  }

  .status-row .value {
    color: var(--ink);
  }

  .status-row .value.success {
    color: var(--success, #22c55e);
  }

  .actions {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }

  .btn {
    padding: 10px 20px;
    border-radius: 12px;
    border: none;
    font-weight: 600;
    font-size: 14px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--accent);
    color: white;
  }

  .btn-danger {
    background: var(--danger);
    color: white;
  }

  .loading {
    color: var(--muted);
    font-size: 13px;
  }

  .info-card ul {
    margin: 0;
    padding-left: 20px;
  }

  .info-card li {
    font-size: 13px;
    margin-bottom: 8px;
    color: var(--ink);
  }

  .info-card li:last-child {
    margin-bottom: 0;
  }

  code {
    background: rgba(255, 255, 255, 0.08);
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 12px;
    font-family: monospace;
  }

  :global([data-theme='light']) code {
    background: rgba(0, 0, 0, 0.06);
  }
</style>
