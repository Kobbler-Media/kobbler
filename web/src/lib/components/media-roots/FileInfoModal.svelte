<script lang="ts">
  import type { MediaRootFile } from '$lib/types';
  import { formatSize } from '$lib/utils/format';

  interface Props {
    file: MediaRootFile;
    onClose: () => void;
  }

  let { file, onClose }: Props = $props();

  const STATUS_LABELS: Record<string, string> = {
    pending: 'Pending',
    identified: 'Identified',
    added: 'Added',
    review: 'Ready to Add',
    ambiguous: 'Ambiguous',
    unmatched: 'Unmatched',
    ignored: 'Ignored'
  };

  const interp = $derived(file.interpretation);
  const mediaType = $derived((interp?.media_type as string) ?? null);
  const candidates = $derived(file.match_candidates ?? []);
  const sidecar = $derived(file.sidecar_metadata);

  type FieldDef = { label: string; key: string; format?: 'list' | 'badge' };

  const BOOK_FIELDS: FieldDef[] = [
    { label: 'Source', key: 'source', format: 'badge' },
    { label: 'Title', key: 'title' },
    { label: 'Authors', key: 'authors', format: 'list' },
    { label: 'ISBN', key: 'isbn' },
    { label: 'Series', key: 'series' },
    { label: 'Series Index', key: 'series_index' },
    { label: 'Publisher', key: 'publisher' },
    { label: 'Language', key: 'language' },
    { label: 'Description', key: 'description' }
  ];

  const TV_FIELDS: FieldDef[] = [
    { label: 'Show', key: 'show_name' },
    { label: 'Year', key: 'show_year' },
    { label: 'Season', key: 'season' },
    { label: 'Episodes', key: 'episodes', format: 'list' },
    { label: 'Episode Title', key: 'episode_title' }
  ];

  const MOVIE_FIELDS: FieldDef[] = [
    { label: 'Title', key: 'movie_title' },
    { label: 'Year', key: 'movie_year' }
  ];

  function fieldsForType(type: string | null): FieldDef[] {
    if (type === 'book' || type === 'audiobook') return BOOK_FIELDS;
    if (type === 'tv') return TV_FIELDS;
    if (type === 'movie') return MOVIE_FIELDS;
    return [];
  }

  function formatValue(val: unknown, format?: 'list' | 'badge'): string {
    if (val == null) return '';
    if (format === 'list' && Array.isArray(val)) return val.join(', ');
    return String(val);
  }

  function getSidecarIds(): [string, string][] {
    if (!interp?.sidecar_ids || typeof interp.sidecar_ids !== 'object') return [];
    const ids = interp.sidecar_ids as Record<string, unknown>;
    return Object.entries(ids).filter(([, v]) => v != null && v !== '') as [string, string][];
  }

  function getSidecarEntries(): [string, unknown][] {
    if (!sidecar || typeof sidecar !== 'object') return [];
    return Object.entries(sidecar).filter(([, v]) => v != null && v !== '');
  }

  function formatSidecarValue(val: unknown): string {
    if (val == null) return '';
    if (typeof val === 'object') return JSON.stringify(val, null, 2);
    return String(val);
  }

  let showRawJson = $state(false);

  function handleBackdropClick() {
    onClose();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <div class="dialog-header">
      <h3>File Details</h3>
      <button class="close-btn" type="button" onclick={onClose}>
        <span class="material-icons-outlined">close</span>
      </button>
    </div>

    <!-- File Info -->
    <div class="section">
      <div class="section-label">File</div>
      <div class="info-card">
        <div class="info-row">
          <span class="info-label">Path</span>
          <span class="info-value mono">{file.file_path}</span>
        </div>
        <div class="info-row">
          <span class="info-label">Size</span>
          <span class="info-value"
            >{file.file_size != null ? formatSize(file.file_size) : '--'}</span
          >
        </div>
        <div class="info-row">
          <span class="info-label">Status</span>
          <span class="info-value">{STATUS_LABELS[file.match_status] ?? file.match_status}</span>
        </div>
        <div class="info-row">
          <span class="info-label">Available</span>
          <span class="info-value">{file.available ? 'Yes' : 'No'}</span>
        </div>
      </div>
    </div>

    <!-- Match Result -->
    {#if file.entity_type && file.entity_id}
      <div class="section">
        <div class="section-label">Match Result</div>
        <div class="info-card">
          <div class="info-row">
            <span class="info-label">Entity Type</span>
            <span class="info-value">{file.entity_type}</span>
          </div>
          <div class="info-row">
            <span class="info-label">Entity ID</span>
            <span class="info-value mono">{file.entity_id}</span>
          </div>
        </div>
      </div>
    {/if}

    <!-- Interpretation -->
    {#if interp}
      <div class="section">
        <div class="section-label">
          Interpretation {mediaType ? `(${mediaType})` : ''}
        </div>
        <div class="info-card">
          {#each fieldsForType(mediaType) as field (field.key)}
            {@const val = interp[field.key]}
            {#if val != null && val !== '' && !(Array.isArray(val) && val.length === 0)}
              <div class="info-row">
                <span class="info-label">{field.label}</span>
                {#if field.format === 'badge'}
                  <span class="info-value">
                    <span class="source-badge">{formatValue(val, field.format)}</span>
                  </span>
                {:else}
                  <span class="info-value">{formatValue(val, field.format)}</span>
                {/if}
              </div>
            {/if}
          {/each}

          <!-- Title conflict indicators -->
          {#if interp?.is_title_conflict}
            <div class="info-row">
              <span class="info-label">
                <span class="conflict-badge">Conflict</span>
              </span>
              <span class="info-value conflict-note">Embedded and filename titles disagree</span>
            </div>
            {#if interp.probe_title}
              <div class="info-row">
                <span class="info-label">Embedded title</span>
                <span class="info-value">{interp.probe_title}</span>
              </div>
            {/if}
            {#if interp.interpret_title}
              <div class="info-row">
                <span class="info-label">Filename title</span>
                <span class="info-value">{interp.interpret_title}</span>
              </div>
            {/if}
          {/if}
          {#if interp?.is_generic_filename}
            <div class="info-row">
              <span class="info-label">
                <span class="generic-badge">Generic</span>
              </span>
              <span class="info-value conflict-note">Title derived from directory name</span>
            </div>
          {/if}

          <!-- External IDs -->
          {#if getSidecarIds().length > 0}
            <div class="info-row ids-section">
              <span class="info-label">External IDs</span>
              <span class="info-value ids-list">
                {#each getSidecarIds() as [key, val] (key)}
                  <span class="id-tag">
                    <span class="id-key">{key}</span>
                    <span class="id-val">{val}</span>
                  </span>
                {/each}
              </span>
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Sidecar Metadata -->
    {#if sidecar && getSidecarEntries().length > 0}
      <div class="section">
        <div class="section-label">Sidecar Metadata</div>
        <div class="info-card">
          {#each getSidecarEntries() as [key, val] (key)}
            <div class="info-row">
              <span class="info-label">{key}</span>
              {#if typeof val === 'object'}
                <span class="info-value mono pre">{formatSidecarValue(val)}</span>
              {:else}
                <span class="info-value">{formatSidecarValue(val)}</span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Match Candidates -->
    {#if candidates.length > 0}
      <div class="section">
        <div class="section-label">Match Candidates ({candidates.length})</div>
        <div class="candidates">
          {#each candidates as candidate (candidate.provider + ':' + candidate.provider_id)}
            <div class="candidate-card">
              {#if candidate.poster_url || candidate.cover_url}
                <img class="poster" src={candidate.poster_url ?? candidate.cover_url} alt="" />
              {:else}
                <div class="poster placeholder">
                  <span class="material-icons-outlined">description</span>
                </div>
              {/if}
              <div class="candidate-info">
                <div class="candidate-title">{candidate.title}</div>
                <div class="candidate-meta">
                  <span class="provider-tag">{candidate.provider}</span>
                  {#if candidate.confidence != null}
                    <span class="confidence">{Math.round(candidate.confidence * 100)}%</span>
                  {/if}
                  {#if candidate.year}
                    <span>{candidate.year}</span>
                  {/if}
                </div>
                {#if candidate.authors && candidate.authors.length > 0}
                  <div class="candidate-authors">{candidate.authors.join(', ')}</div>
                {/if}
                {#if candidate.isbn}
                  <div class="candidate-isbn">ISBN: {candidate.isbn}</div>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Raw JSON toggle -->
    {#if interp || sidecar}
      <div class="section">
        <button class="raw-toggle" type="button" onclick={() => (showRawJson = !showRawJson)}>
          <span class="material-icons-outlined">{showRawJson ? 'expand_less' : 'expand_more'}</span>
          Raw JSON
        </button>
        {#if showRawJson}
          <pre class="raw-json">{JSON.stringify(
              { interpretation: interp, sidecar_metadata: sidecar, match_candidates: candidates },
              null,
              2
            )}</pre>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dialog {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    max-width: 560px;
    width: 90vw;
    max-height: 85vh;
    overflow-y: auto;
    padding: 20px;
    position: relative;
  }

  .dialog-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
  }

  .dialog-header h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    color: var(--ink);
  }

  .close-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .close-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .section {
    margin-bottom: 16px;
  }

  .section:last-child {
    margin-bottom: 0;
  }

  .section-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .info-card {
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 10px;
  }

  .info-row {
    display: flex;
    gap: 12px;
    padding: 4px 0;
    align-items: baseline;
  }

  .info-row + .info-row {
    border-top: 1px solid color-mix(in srgb, var(--ring) 40%, transparent);
  }

  .info-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    min-width: 90px;
    flex-shrink: 0;
  }

  .info-value {
    font-size: 13px;
    color: var(--ink);
    word-break: break-all;
    min-width: 0;
  }

  .info-value.mono {
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
    font-size: 12px;
  }

  .info-value.pre {
    white-space: pre-wrap;
    font-size: 11px;
    color: var(--muted);
  }

  .conflict-badge,
  .generic-badge {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.3px;
  }

  .conflict-badge {
    background: rgba(251, 191, 36, 0.15);
    color: #fbbf24;
  }

  .generic-badge {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
  }

  .conflict-note {
    font-size: 12px;
    color: var(--muted);
    font-style: italic;
  }

  .source-badge {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    background: rgba(96, 165, 250, 0.15);
    color: #60a5fa;
  }

  .ids-section {
    flex-direction: column;
    gap: 6px;
  }

  .ids-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .id-tag {
    display: inline-flex;
    align-items: center;
    border-radius: 4px;
    overflow: hidden;
    font-size: 11px;
    border: 1px solid var(--ring);
  }

  .id-key {
    padding: 1px 5px;
    background: var(--bg-2);
    color: var(--muted);
    font-weight: 600;
    text-transform: uppercase;
    font-size: 9px;
    letter-spacing: 0.3px;
  }

  .id-val {
    padding: 1px 6px;
    color: var(--ink);
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
    font-size: 11px;
  }

  .candidates {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 240px;
    overflow-y: auto;
  }

  .candidate-card {
    display: flex;
    gap: 10px;
    padding: 10px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg);
  }

  .poster {
    width: 44px;
    height: 64px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .poster.placeholder {
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 20px;
    opacity: 0.4;
  }

  .candidate-info {
    flex: 1;
    min-width: 0;
  }

  .candidate-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .candidate-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .provider-tag {
    font-size: 9px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--accent);
    color: #1d130c;
    text-transform: uppercase;
  }

  .confidence {
    font-size: 10px;
    font-weight: 600;
    color: var(--accent);
  }

  .candidate-authors,
  .candidate-isbn {
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .raw-toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    border: 1px solid var(--ring);
    background: none;
    color: var(--muted);
    cursor: pointer;
    margin-bottom: 8px;
  }

  .raw-toggle:hover {
    color: var(--ink);
  }

  .raw-toggle :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .raw-json {
    padding: 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
    font-size: 11px;
    color: var(--muted);
    overflow-x: auto;
    white-space: pre;
    line-height: 1.4;
    max-height: 300px;
    overflow-y: auto;
  }
</style>
