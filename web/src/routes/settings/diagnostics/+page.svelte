<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api/client';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type { PlaybackReport } from '$lib/types';

  let reports = $state<PlaybackReport[]>([]);
  let status = $state<'loading' | 'ready' | 'error'>('loading');
  let error = $state('');
  let expanded = $state<string | null>(null);

  const load = async () => {
    status = 'loading';
    try {
      const result = await api.getPlaybackReports();
      reports = result.items ?? [];
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load playback reports';
      status = 'error';
    }
  };

  onMount(load);

  const kindLabel = (kind: string) => {
    switch (kind) {
      case 'android':
        return 'Android';
      case 'androidtv':
        return 'Android TV';
      case 'desktop':
        return 'Desktop app';
      case 'ios':
        return 'iOS';
      case 'web':
        return 'Browser';
      default:
        return kind;
    }
  };

  const relative = (iso: string) => {
    const delta = Date.now() - new Date(iso).getTime();
    const mins = Math.floor(delta / 60_000);
    if (mins < 1) return 'just now';
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.floor(hours / 24)}d ago`;
  };

  // The report opens with "Playback failed" and a context block; the line worth
  // showing collapsed is the exception itself, which is the first one naming a type.
  const headline = (detail: string) => {
    const line = detail
      .split('\n')
      .map((l) => l.trim())
      .find((l) => l.includes('Exception') || l.includes('Error'));
    return line || detail.split('\n')[0] || 'Playback failed';
  };

  // Direct `.ts` vs an HLS playlist is the first thing a playback bug turns on.
  const pathLabel = (url: string | null) => {
    if (!url) return null;
    if (url.includes('/stream.ts')) return 'direct';
    if (url.includes('.m3u8')) return 'HLS';
    return null;
  };
</script>

<svelte:head>
  <title>Playback reports — Kobbler</title>
</svelte:head>

<div class="reports-section">
  <div class="section-header">
    <h2>Playback Reports</h2>
    <div class="header-actions">
      <span class="muted">{reports.length} recent</span>
      <button class="btn alt small" type="button" onclick={load}>Refresh</button>
    </div>
  </div>

  <p class="muted intro">
    Clients report a failed playback on their own. Each one carries what the server cannot see — the
    URL the player was actually handed, the device, the app build — so a broken stream leaves a
    trace without anyone having to describe it.
  </p>

  {#if status === 'loading'}
    <div class="muted">Loading reports…</div>
  {:else if status === 'error'}
    <div class="error">{error}</div>
  {:else if reports.length === 0}
    <div class="empty-state">
      <h3>Nothing reported</h3>
      <p class="muted">Playback has not failed on any client since this was switched on.</p>
    </div>
  {:else}
    <div class="report-list">
      {#each reports as report (report.id)}
        {@const path = pathLabel(report.stream_url)}
        <div class="report-row">
          <button
            class="report-head"
            type="button"
            onclick={() => (expanded = expanded === report.id ? null : report.id)}
            aria-expanded={expanded === report.id}
          >
            <div class="report-info">
              <div class="report-title">
                {headline(report.detail)}
                {#if path}<span class="pill">{path}</span>{/if}
              </div>
              <div class="report-meta muted">
                <MetaLine
                  items={[
                    kindLabel(report.client_kind),
                    report.device_label,
                    report.app_version && `v${report.app_version}`,
                    relative(report.created_at)
                  ]}
                />
              </div>
              {#if report.media_type}
                <div class="report-meta muted">
                  <MetaLine items={[report.media_type, report.media_id]} />
                </div>
              {/if}
            </div>
            <span class="chevron material-icons-outlined">
              {expanded === report.id ? 'expand_less' : 'expand_more'}
            </span>
          </button>

          {#if expanded === report.id}
            <pre class="detail">{report.detail}</pre>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .reports-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .intro {
    margin: 0;
    max-width: 68ch;
  }

  .report-list {
    display: grid;
    gap: 8px;
  }

  .report-row {
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    overflow: hidden;
  }

  .report-head {
    display: flex;
    align-items: center;
    gap: 14px;
    width: 100%;
    padding: 14px 16px;
    background: none;
    border: none;
    color: inherit;
    text-align: left;
    cursor: pointer;
    font: inherit;
  }

  .report-head:hover {
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }

  .report-info {
    display: grid;
    gap: 4px;
    flex-grow: 1;
    min-width: 0;
  }

  .report-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    font-size: 13.5px;
    overflow-wrap: anywhere;
  }

  .report-meta {
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .pill {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 2px 7px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: var(--accent);
    flex-shrink: 0;
  }

  .chevron {
    font-size: 20px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .detail {
    margin: 0;
    padding: 14px 16px;
    border-top: 1px solid var(--ring);
    background: var(--bg);
    font-size: 12px;
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 420px;
    overflow-y: auto;
  }
</style>
