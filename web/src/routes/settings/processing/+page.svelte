<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import {
    getSystemConfig,
    updateSystemConfig,
    triggerEnrichmentBackfill,
    rerunSkipDetect,
    runIntegrityCheck,
    getVisionHealth,
    type SystemConfig,
    type SystemConfigUpdate
  } from '$lib/api/client';
  import type { MediaScope, VisionHealthSnapshot } from '$lib/types';

  let systemConfig = $state<SystemConfig | null>(null);
  let systemConfigLoading = $state(true);
  let systemConfigError = $state<string | null>(null);
  let systemConfigSaving = $state(false);
  let backfilling = $state(false);
  let rerunningSkipDetect = $state<null | 'normal' | 'force'>(null);
  let checkingIntegrity = $state(false);
  let visionHealth = $state<VisionHealthSnapshot | null>(null);

  // Both passes fan out per file and run a SigLIP2 classify on each, and the
  // library holds far more episode files than movie files — so scoping a run
  // to the kind whose handling actually changed is the difference between
  // minutes and a full-library GPU pass.
  type BackfillScope = 'all' | MediaScope;
  type SkipDetectScope = 'all' | 'movie' | 'episode';
  let backfillScope = $state<BackfillScope>('all');
  let skipDetectScope = $state<SkipDetectScope>('all');

  const SCOPE_LABELS: Record<MediaScope, string> = {
    movie: 'movies',
    episode: 'TV',
    audiobook: 'audiobooks'
  };

  const auth = getAuthState();

  onMount(async () => {
    await loadSystemConfig();
    try {
      visionHealth = await getVisionHealth();
    } catch {
      visionHealth = null;
    }
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

  const handleEnrichBackfill = async () => {
    if (backfilling) return;
    backfilling = true;
    try {
      await triggerEnrichmentBackfill({
        mediaType: backfillScope === 'all' ? undefined : backfillScope
      });
      showMessage(
        backfillScope === 'all'
          ? 'Enrichment backfill started'
          : `Enrichment backfill started — ${SCOPE_LABELS[backfillScope]} only`
      );
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start enrichment backfill');
    } finally {
      backfilling = false;
    }
  };

  const handleRerunSkipDetect = async (force: boolean) => {
    if (rerunningSkipDetect) return;
    // The force path re-detects everything in scope rather than the out-of-date
    // slice, which is hours instead of minutes. It used to sit behind a
    // checkbox; now that it is a button of its own, confirm instead — naming
    // the scope, since "everything" now depends on the selector above it.
    const target =
      skipDetectScope === 'all' ? 'the entire library' : `all ${SCOPE_LABELS[skipDetectScope]}`;
    if (
      force &&
      !confirm(
        `Re-detect ${target}, including content that is already up to date?\n\n` +
          'This takes considerably longer than the normal re-run. Only needed when the ' +
          'detection logic itself has changed.'
      )
    ) {
      return;
    }
    rerunningSkipDetect = force ? 'force' : 'normal';
    try {
      const r = await rerunSkipDetect(
        force,
        skipDetectScope === 'all' ? undefined : skipDetectScope
      );
      if (r.queued === 0) {
        showMessage('Everything is already up to date — nothing queued');
      } else {
        const parts: string[] = [];
        if (r.seasons > 0) parts.push(`${r.seasons} season${r.seasons === 1 ? '' : 's'}`);
        if (r.movies > 0) parts.push(`${r.movies} movie${r.movies === 1 ? '' : 's'}`);
        showMessage(
          `Queued ${r.queued} job${r.queued === 1 ? '' : 's'} (${parts.join(', ')})` +
            (r.forced ? ' — forced' : '')
        );
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to queue skip detection');
    } finally {
      rerunningSkipDetect = null;
    }
  };

  const handleIntegrityCheck = async () => {
    if (checkingIntegrity) return;
    checkingIntegrity = true;
    try {
      await runIntegrityCheck({ stampOnly: false });
      showMessage('Integrity check queued — findings appear on the Jobs page');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to queue integrity check');
    } finally {
      checkingIntegrity = false;
    }
  };

  const saveSystemConfig = async () => {
    if (!systemConfig) return;
    systemConfigSaving = true;
    systemConfigError = null;
    try {
      const updates: SystemConfigUpdate = {
        enrichment_enabled: systemConfig.enrichment_enabled,
        skip_detect_mode: systemConfig.skip_detect_mode,
        vision_service_url: systemConfig.vision_service_url
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
  <title>Processing Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="processing-section">
    <div class="section-header">
      <h2>Processing</h2>
    </div>

    {#if systemConfigLoading}
      <div class="loading-card">Loading configuration...</div>
    {:else if systemConfigError}
      <div class="error">{systemConfigError}</div>
    {:else if systemConfig}
      <div class="config-form">
        <div class="config-card">
          <h3>Media Enrichment</h3>
          <div data-field="enrichment_enabled">
            <label class="checkbox-label">
              <input type="checkbox" bind:checked={systemConfig.enrichment_enabled} />
              Enable media enrichment
            </label>
            <span class="field-hint"
              >Generate seek bar thumbnails and measure loudness (LUFS) for video and audio files.</span
            >
          </div>
          <div class="action-list">
            <div class="action">
              <label class="scope-field">
                Scope
                <select bind:value={backfillScope} disabled={backfilling}>
                  <option value="all">All media</option>
                  <option value="movie">Movies only</option>
                  <option value="episode">TV only</option>
                  <option value="audiobook">Audiobooks only</option>
                </select>
              </label>
              <button
                type="button"
                class="btn-action"
                disabled={backfilling}
                onclick={handleEnrichBackfill}
              >
                {backfilling ? 'Running…' : 'Run Enrichment Backfill Now'}
              </button>
              <span class="field-hint">
                Analyses any file that has not been through the current pipeline. One decode per
                file produces the seek-bar thumbnails and loudness above, and also the fingerprints
                skip detection reads — so this is the slow pass, and it runs on its own every 30
                minutes anyway. Only files below the current pipeline version are touched, so on an
                up-to-date library it queues nothing. Scope it when only one kind of media needs
                re-analysing — the library holds far more episodes than movies, and this decodes
                every file it touches.
              </span>
            </div>

            <div class="action">
              <button
                type="button"
                class="btn-action"
                disabled={checkingIntegrity}
                onclick={handleIntegrityCheck}
              >
                {checkingIntegrity ? 'Queuing…' : 'Check Media Integrity'}
              </button>
              <span class="field-hint">
                Looks for files that are not what kobbler thinks they are: a cache that no longer
                matches its source, or the empty blocks an unfinished download leaves behind. A
                cheap version of this runs daily on its own; this button also samples file contents,
                reading a little from every file. Reports only — nothing is deleted.
              </span>
            </div>
          </div>
        </div>

        <div class="config-card">
          <h3>Skip Detection</h3>
          <p class="field-hint" style="margin-bottom: 4px;">
            Detect intro sequences, credits, and post-credits scenes in TV shows and movies. Uses
            audio fingerprinting and optional visual analysis.
          </p>

          <div data-field="skip_detect_mode">
            <div class="field">
              <label for="skip-detect-mode">Detection Mode</label>
              <select
                id="skip-detect-mode"
                bind:value={systemConfig.skip_detect_mode}
                onchange={saveSystemConfig}
              >
                <option value="off">Off</option>
                <option value="audio">Audio Only (Chromaprint + boundary detection)</option>
                <option value="full">Full (adds SigLIP2 visual analysis)</option>
              </select>
            </div>
          </div>

          {#if visionHealth && (visionHealth.status === 'failing' || visionHealth.status === 'degraded')}
            <div class="vision-alert" class:failing={visionHealth.status === 'failing'}>
              <span class="material-icons-outlined">warning</span>
              <div>
                <strong>
                  {visionHealth.status === 'failing'
                    ? 'Vision embedding is failing.'
                    : 'Vision embedding is degrading.'}
                </strong>
                <div>{visionHealth.note}</div>
                {#if visionHealth.last_error}
                  <div class="vision-alert-error">{visionHealth.last_error}</div>
                {/if}
              </div>
            </div>
          {:else if visionHealth && visionHealth.status === 'healthy'}
            <p class="field-hint">
              Vision embedding healthy — {visionHealth.ok} of the last {visionHealth.attempted}
              attempts succeeded.
            </p>
          {/if}

          <div data-field="vision_service_url">
            <div class="field">
              <label for="vision-url">Vision Service URL</label>
              <input
                id="vision-url"
                type="text"
                bind:value={systemConfig.vision_service_url}
                onchange={saveSystemConfig}
                placeholder="http://siglip2:8890"
              />
              <span class="field-hint"
                >Required for "Full" mode. Points at the SigLIP2 service (compose service
                <code>siglip2</code>){systemConfig.koblem_ui_enabled
                  ? ', or at koblem when GPU routing is enabled'
                  : ''}.</span
              >
            </div>
          </div>

          {#if systemConfig.skip_detect_mode !== 'off'}
            <div class="action-list">
              <div class="action">
                <label class="scope-field">
                  Scope
                  <select bind:value={skipDetectScope} disabled={rerunningSkipDetect !== null}>
                    <option value="all">All media</option>
                    <option value="movie">Movies only</option>
                    <option value="episode">TV only</option>
                  </select>
                </label>
                <span class="field-hint">
                  Applies to both buttons below. Detection runs a visual credit-text pass per file
                  on both the TV and movie paths, and the library holds far more episode files than
                  movie files — so narrow this to the kind whose detection actually changed rather
                  than re-running everything.
                </span>
              </div>

              <div class="action">
                <button
                  type="button"
                  class="btn-action"
                  disabled={rerunningSkipDetect !== null}
                  onclick={() => handleRerunSkipDetect(false)}
                >
                  {rerunningSkipDetect === 'normal' ? 'Queuing…' : 'Re-run Skip Detection'}
                </button>
                <span class="field-hint">
                  Queues only what is out of date — content whose analysis cache is newer than its
                  last detection. Reads the cached analysis, so nothing is re-decoded. One job per
                  TV season (detection compares every episode in a season against the others) plus
                  one per movie. Safe to press any time: when everything is current it queues
                  nothing, and an interrupted run picks up where it left off.
                </span>
              </div>

              <div class="action">
                <button
                  type="button"
                  class="btn-action"
                  disabled={rerunningSkipDetect !== null}
                  onclick={() => handleRerunSkipDetect(true)}
                >
                  {rerunningSkipDetect === 'force' ? 'Queuing…' : 'Force Re-detect Everything'}
                </button>
                <span class="field-hint">
                  Ignores the up-to-date check and re-detects everything in scope. Needed when the
                  detection logic itself has changed — a timestamp comparison cannot see that. Takes
                  considerably longer, so prefer the button above unless you know detection changed,
                  and set the scope above to just the kind that changed.
                </span>
              </div>
            </div>
          {/if}
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

  .processing-section {
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
  .config-card select {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .config-card input:disabled,
  .config-card select:disabled {
    opacity: 0.5;
  }

  .field {
    display: grid;
    gap: 6px;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  /* Stacked actions: each is a button with its explanation directly beneath it.
     This replaced a flex row, which laid button and description out side by
     side as narrow columns — buttons squeezed to their minimum width and the
     text reflowed into tall thin strips. */
  .action-list {
    display: grid;
    gap: 16px;
    padding-top: 4px;
  }

  .action {
    display: grid;
    gap: 6px;
    justify-items: start;
  }

  .action .field-hint {
    max-width: 62ch;
    line-height: 1.5;
  }

  /* Qualifier for the action beneath it. Inherits the label-over-control shape
     and the select chrome from the configuration fields above; these overrides
     only size it down to sit with `.btn-action` rather than with the
     full-width form selects. Both need to out-specify
     `.config-card label:not(.checkbox-label)` and `.config-card select`. */
  .config-card label.scope-field {
    gap: 4px;
    font-size: 11px;
  }

  .config-card .scope-field select {
    padding: 5px 8px;
    font-size: 12px;
  }

  .btn-action {
    padding: 6px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-action:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn-action:disabled {
    opacity: 0.4;
    cursor: not-allowed;
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

  .vision-alert {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px 14px;
    margin: 12px 0;
    border-radius: 12px;
    border: 1px solid var(--warning);
    background: color-mix(in srgb, var(--warning) 12%, transparent);
    font-size: 13px;
    line-height: 1.5;
  }

  .vision-alert.failing {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }

  .vision-alert .material-icons-outlined {
    color: var(--warning);
    flex-shrink: 0;
  }

  .vision-alert.failing .material-icons-outlined {
    color: var(--danger);
  }

  .vision-alert-error {
    margin-top: 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    opacity: 0.85;
    word-break: break-word;
  }
</style>
