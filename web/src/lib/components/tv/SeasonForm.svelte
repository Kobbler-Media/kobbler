<script lang="ts">
  import type { TvSeason, SeasonDetailResult } from '#lib/types.js';
  import SourceField from '#lib/components/SourceField.svelte';
  import CoverSelector from '#lib/components/CoverSelector.svelte';
  import {
    buildVideoSourceValues,
    getVideoInitialByPriority,
    getVideoInitialSourceForField,
    buildVideoCoverOptions
  } from '#lib/components/video/video-form-utils.js';

  export interface SeasonFormData {
    season_identifier: string;
    season_sort_order: number;
    title?: string | null;
    description?: string | null;
    air_date?: string | null;
    episode_count?: number | null;
    is_special?: boolean;
    poster_url?: string | null;
  }

  interface Props {
    season?: TvSeason | null;
    searchResults?: SeasonDetailResult[];
    onSave: (data: SeasonFormData, posterFile?: File) => Promise<void>;
    onCancel: () => void;
    onDelete?: () => void;
    saving?: boolean;
    deleting?: boolean;
    hideFooter?: boolean;
    requestSave?: () => void;
    canSave?: boolean;
  }

  let {
    season = null,
    searchResults,
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  let season_identifier = $state('');
  let season_sort_order = $state<number>(0);
  let title = $state('');
  let description = $state('');
  let air_date = $state('');
  let episode_count = $state<number | null>(null);
  let is_special = $state(false);

  // Source tracking
  let titleSource = $state<string | undefined>(undefined);
  let descriptionSource = $state<string | undefined>(undefined);
  let airDateSource = $state<string | undefined>(undefined);

  // Cover state
  let selectedPosterUrl = $state<string | null>(null);
  let posterFile = $state<File | undefined>(undefined);

  // Build source values from search results
  const titleSources = $derived(buildVideoSourceValues('title', searchResults as any[]));
  const descriptionSources = $derived(
    buildVideoSourceValues('description', searchResults as any[])
  );
  const airDateSources = $derived(buildVideoSourceValues('air_date', searchResults as any[]));
  const posterOptions = $derived(buildVideoCoverOptions(searchResults as any[], 'poster_url'));

  $effect(() => {
    if (season) {
      season_identifier = season.season_identifier || '';
      season_sort_order = season.season_sort_order ?? 0;
      title = season.title || '';
      description = season.description || '';
      air_date = season.air_date || '';
      episode_count = season.episode_count ?? null;
      is_special = season.is_special ?? false;
      selectedPosterUrl = season.poster_url || null;
    } else if (searchResults && searchResults.length > 0) {
      title = getVideoInitialByPriority('title', searchResults as any[]);
      titleSource = getVideoInitialSourceForField('title', searchResults as any[]);
      description = getVideoInitialByPriority('description', searchResults as any[]);
      descriptionSource = getVideoInitialSourceForField('description', searchResults as any[]);
      air_date = getVideoInitialByPriority('air_date', searchResults as any[]);
      airDateSource = getVideoInitialSourceForField('air_date', searchResults as any[]);
      selectedPosterUrl = posterOptions[0]?.url || null;
    } else {
      season_identifier = '';
      season_sort_order = 0;
      title = '';
      description = '';
      air_date = '';
      episode_count = null;
      is_special = false;
      selectedPosterUrl = null;
    }
  });

  $effect(() => {
    canSave = season_identifier.trim().length > 0;
  });

  async function handleSave() {
    if (!canSave) return;
    const formData: SeasonFormData = {
      season_identifier: season_identifier.trim(),
      season_sort_order,
      title: title.trim() || null,
      description: description.trim() || null,
      air_date: air_date.trim() || null,
      episode_count: episode_count ?? null,
      is_special
    };
    // Include image URL in metadata so the backend can lazy-cache it
    if (!posterFile && selectedPosterUrl && selectedPosterUrl !== season?.poster_url) {
      formData.poster_url = selectedPosterUrl;
    }
    await onSave(formData, posterFile || undefined);
  }

  $effect(() => {
    requestSave = handleSave;
  });
</script>

<div class="season-form">
  <div class="form-grid">
    <div class="form-field">
      <label for="season_identifier">Season Number*</label>
      <input
        id="season_identifier"
        type="text"
        bind:value={season_identifier}
        placeholder="e.g. 1, 2, 0 for specials"
        disabled={!!season}
      />
    </div>

    <div class="form-field">
      <label for="season_sort_order">Sort Order</label>
      <input id="season_sort_order" type="number" bind:value={season_sort_order} />
    </div>

    <div class="form-field full-width">
      <SourceField
        label="Title"
        value={title}
        sourceValues={titleSources}
        currentSource={titleSource}
        onchange={(val, src) => {
          title = val;
          titleSource = src;
        }}
        type="text"
        placeholder="e.g. Season 1"
      />
    </div>

    <div class="form-field">
      <SourceField
        label="Air Date"
        value={air_date}
        sourceValues={airDateSources}
        currentSource={airDateSource}
        onchange={(val, src) => {
          air_date = val;
          airDateSource = src;
        }}
        type="text"
        placeholder="YYYY-MM-DD"
      />
    </div>

    <div class="form-field">
      <label for="episode_count">Episode Count</label>
      <input id="episode_count" type="number" bind:value={episode_count} placeholder="e.g. 10" />
    </div>

    <div class="form-field full-width">
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={is_special} />
        Special Season
      </label>
    </div>

    <div class="form-field full-width">
      <SourceField
        label="Description"
        value={description}
        sourceValues={descriptionSources}
        currentSource={descriptionSource}
        onchange={(val, src) => {
          description = val;
          descriptionSource = src;
        }}
        type="textarea"
        placeholder="Enter description"
      />
    </div>

    <div class="form-field full-width">
      <CoverSelector
        id="season-poster"
        label="Poster"
        currentUrl={season?.poster_url}
        searchCovers={posterOptions}
        selectedUrl={selectedPosterUrl}
        onSelect={(url, file) => {
          selectedPosterUrl = url;
          posterFile = file;
        }}
        aspectRatio="portrait"
      />
    </div>
  </div>

  {#if !hideFooter}
    <div class="form-footer">
      <div class="footer-left">
        {#if season && onDelete}
          <button type="button" class="btn-danger" onclick={onDelete} disabled={deleting}>
            {deleting ? 'Deleting...' : 'Delete'}
          </button>
        {/if}
      </div>
      <div class="footer-right">
        <button type="button" class="btn-sec" onclick={onCancel} disabled={saving || deleting}>
          Cancel
        </button>
        <button
          type="button"
          class="btn-primary"
          onclick={handleSave}
          disabled={!canSave || saving || deleting}
        >
          {saving ? 'Saving...' : 'Save'}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .season-form {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .form-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 16px;
  }
  .form-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .form-field.full-width {
    grid-column: 1 / -1;
  }
  label {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.5px;
  }
  input {
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    border-radius: 8px;
    padding: 8px 12px;
    font-size: 14px;
    font-family: inherit;
    transition: border-color 0.2s;
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
  }
  input::placeholder {
    color: var(--muted);
    opacity: 0.6;
  }
  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    text-transform: none;
    font-size: 14px;
    font-weight: 400;
    color: var(--ink);
    cursor: pointer;
  }
  .checkbox-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
  }
  .form-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding-top: 16px;
    border-top: 1px solid var(--ring);
  }
  .footer-left,
  .footer-right {
    display: flex;
    gap: 12px;
  }
</style>
