<script lang="ts">
  import type { LivetvSeriesRulePreview } from '#lib/types.js';

  interface Props {
    preview: LivetvSeriesRulePreview | null;
    loading: boolean;
    error: string;
  }

  let { preview, loading, error }: Props = $props();

  function clock(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime())
      ? iso
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  const totals = $derived.by(() => {
    if (!preview) return null;
    const airings = preview.clusters.reduce((n, c) => n + c.members.length, 0);
    return { airings, clusters: preview.clusters.length, record: preview.would_record };
  });
</script>

<section class="preview">
  <header>
    <div class="line">
      <span class="heading">What this records</span>
      <span class="sub">from the live guide, re-evaluated on every refresh</span>
    </div>
    {#if totals}
      <span class="summary">
        <strong>{totals.airings}</strong> airing{totals.airings === 1 ? '' : 's'} in the window across
        <strong>{totals.clusters}</strong>
        cluster{totals.clusters === 1 ? '' : 's'} — <strong>{totals.record}</strong>
        recording{totals.record === 1 ? '' : 's'}, {preview?.suppressed} suppressed.
      </span>
    {/if}
  </header>

  <div class="body">
    {#if loading}
      <div class="loading-card">Re-evaluating against the guide…</div>
    {:else if error}
      <p class="state">{error}</p>
    {:else if !preview}
      <p class="state">Pick a series and at least one channel to see what the rule would record.</p>
    {:else if preview.clusters.length === 0}
      <p class="state">
        Nothing in the guide window matches this rule yet. The guide only reaches 48 hours, so a
        weekly series may simply not be in it.
      </p>
    {:else}
      <div class="clusters">
        {#each preview.clusters as cluster, i (cluster.label + cluster.start_time)}
          <div class="cluster">
            <div class="cluster-head">
              <span class="cluster-name">
                Cluster {String.fromCharCode(65 + i)} · {clock(cluster.start_time)}
              </span>
              <span class="cluster-note">{cluster.label}</span>
            </div>
            <div class="members">
              {#each cluster.members as member (member.guide_id)}
                <div class="member" class:record={member.action === 'record'}>
                  <span class="action" class:record={member.action === 'record'}>
                    {member.action === 'record' ? 'RECORD' : 'SKIP'}
                  </span>
                  <span class="lcn">{member.lcn ?? ''}</span>
                  <span class="channel">{member.channel_name}</span>
                  <span class="quality" class:hd={member.hd}>{member.hd ? 'HD' : 'SD'}</span>
                  <span class="start">{clock(member.start_time)}</span>
                  <span class="reason">{member.reason ?? ''}</span>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <footer>
    <span class="material-icons-outlined">info</span>
    <span>
      Clustering uses a <strong>±5 min tolerance</strong>, not equal start times — simulcast feeds
      drift, and matching exactly would record one broadcast once per feed.
    </span>
  </footer>
</section>

<style>
  .preview {
    flex-grow: 1;
    min-width: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  header {
    padding: 14px 16px 12px;
    border-bottom: 1px solid var(--ring);
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .heading {
    font-size: 14px;
    font-weight: 600;
  }

  .sub {
    font-size: 11.5px;
    color: var(--muted);
  }

  .summary {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.5;
  }

  .summary strong {
    color: var(--ink);
  }

  .body {
    flex-grow: 1;
    overflow-y: auto;
    padding: 14px 16px;
  }

  .state {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
    line-height: 1.6;
  }

  .clusters {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .cluster {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .cluster-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .cluster-name {
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--accent);
    flex-shrink: 0;
  }

  .cluster-note {
    font-size: 11.5px;
    color: var(--muted);
  }

  .members {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .member {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 11px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid #23272e;
  }

  .member.record {
    background: rgba(34, 197, 94, 0.1);
    border-color: var(--success);
  }

  .action {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    color: #5f6874;
    border: 1px solid var(--ring);
    border-radius: 4px;
    padding: 2px 7px;
    flex-shrink: 0;
  }

  .action.record {
    color: var(--bg);
    background: var(--success);
    border-color: var(--success);
  }

  .lcn {
    width: 26px;
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .channel {
    width: 132px;
    flex-shrink: 0;
    font-size: 12px;
    font-weight: 600;
    color: #6d7681;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .member.record .channel {
    color: var(--ink);
  }

  .quality {
    font-size: 9.5px;
    font-weight: 700;
    color: #6d7681;
    border: 1px solid var(--ring);
    border-radius: 3px;
    padding: 1px 5px;
    flex-shrink: 0;
  }

  .quality.hd {
    color: #5fcf8a;
    border-color: #5fcf8a;
  }

  .start {
    width: 58px;
    flex-shrink: 0;
    font-size: 11.5px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .reason {
    font-size: 11px;
    color: #5f6874;
    font-style: italic;
    flex-grow: 1;
    text-align: right;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  footer {
    border-top: 1px solid var(--ring);
    padding: 11px 16px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 11.5px;
    color: var(--muted);
    line-height: 1.5;
  }

  footer strong {
    color: var(--ink);
  }

  footer .material-icons-outlined {
    font-size: 15px;
    color: var(--accent);
    flex-shrink: 0;
  }

  :global([data-theme='light']) .member {
    background: rgba(0, 0, 0, 0.02);
    border-color: var(--ring);
  }
</style>
