<script lang="ts">
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getBackupUrl,
    restoreBackup,
    triggerOrphanCleanup,
    type RestoreResult
  } from '$lib/api/client';

  const auth = getAuthState();

  let includeProxyBlobs = $state(false);
  let backupInProgress = $state(false);
  let restoreInProgress = $state(false);
  let restoreFiles: FileList | null = $state(null);
  let cleanupInProgress = $state(false);

  const handleBackup = () => {
    backupInProgress = true;
    const url = getBackupUrl(includeProxyBlobs);
    const link = document.createElement('a');
    link.href = url;
    link.download = '';
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    setTimeout(() => {
      backupInProgress = false;
      showMessage('Backup download started');
    }, 1000);
  };

  const handleCleanup = async () => {
    cleanupInProgress = true;
    try {
      const result = await triggerOrphanCleanup();
      showMessage(`Orphan cleanup queued (job ${result.job_id.slice(0, 8)}…)`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to trigger cleanup');
    } finally {
      cleanupInProgress = false;
    }
  };

  const handleRestore = async () => {
    if (!restoreFiles || restoreFiles.length === 0) {
      showMessage('Please select a backup file');
      return;
    }

    const confirmed = confirm(
      'This will replace ALL existing data with the backup contents. This action cannot be undone. Are you sure?'
    );
    if (!confirmed) return;

    restoreInProgress = true;
    try {
      const result: RestoreResult = await restoreBackup(restoreFiles[0]);
      showMessage(
        `Restore complete: ${result.stats.books_restored} books, ${result.stats.files_restored} files`
      );
      setTimeout(() => {
        window.location.reload();
      }, 1500);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Restore failed');
      restoreInProgress = false;
    }
  };
</script>

<svelte:head>
  <title>Backup — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="backup-section">
    <div class="section-header">
      <h2>Backup & Restore</h2>
    </div>

    <div class="backup-card">
      <h3>Create Backup</h3>
      <p class="muted">
        Download a backup of your library including all books, metadata, user data, and settings.
      </p>
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={includeProxyBlobs} />
        Include proxy blobs (large!)
      </label>
      <button class="btn" type="button" onclick={handleBackup} disabled={backupInProgress}>
        {backupInProgress ? 'Preparing…' : 'Download Backup'}
      </button>
    </div>

    <div class="restore-card">
      <h3>Restore from Backup</h3>
      <p class="muted warning">
        Warning: Restoring a backup will replace all existing data. This cannot be undone.
      </p>
      <label class="file-label">
        Select backup file (.zip)
        <input
          type="file"
          accept=".zip"
          onchange={(e) => (restoreFiles = (e.target as HTMLInputElement).files)}
          disabled={restoreInProgress}
        />
      </label>
      {#if restoreFiles && restoreFiles.length > 0}
        <div class="file-selected muted">
          Selected: {restoreFiles[0].name} ({(restoreFiles[0].size / 1024 / 1024).toFixed(2)} MB)
        </div>
      {/if}
      <button
        class="btn btn-danger"
        type="button"
        onclick={handleRestore}
        disabled={restoreInProgress || !restoreFiles || restoreFiles.length === 0}
      >
        {restoreInProgress ? 'Restoring…' : 'Restore Backup'}
      </button>
    </div>

    <div class="cleanup-card">
      <h3>Orphan Cleanup</h3>
      <p class="muted">
        Remove orphaned files from storage — cached images, covers, and book file directories that
        no longer belong to any entity in the database.
      </p>
      <button class="btn" type="button" onclick={handleCleanup} disabled={cleanupInProgress}>
        {cleanupInProgress ? 'Queuing…' : 'Run Cleanup'}
      </button>
    </div>
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

  .backup-section {
    display: grid;
    gap: 16px;
    max-width: 600px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .backup-card,
  .restore-card,
  .cleanup-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 20px;
    display: grid;
    gap: 12px;
  }

  .backup-card h3,
  .restore-card h3,
  .cleanup-card h3 {
    margin: 0;
    font-size: 16px;
  }

  .backup-card p,
  .restore-card p,
  .cleanup-card p {
    margin: 0;
    font-size: 13px;
  }

  .restore-card .warning {
    color: var(--danger);
  }

  .file-label {
    display: grid;
    gap: 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .file-label input[type='file'] {
    padding: 8px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .file-selected {
    font-size: 12px;
  }

  .btn {
    padding: 10px 20px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 14px;
    cursor: pointer;
    justify-self: start;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger {
    background: var(--danger);
  }
</style>
