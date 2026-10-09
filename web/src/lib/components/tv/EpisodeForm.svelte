<script lang="ts">
  import type { TvEpisode, EpisodeDetailResult } from '$lib/types';
  import SourceField from '$lib/components/SourceField.svelte';
  import CoverSelector from '$lib/components/CoverSelector.svelte';
  import {
    buildVideoSourceValues,
    getVideoInitialByPriority,
    getVideoInitialSourceForField,
    buildVideoCoverOptions
  } from '$lib/components/video/video-form-utils';

  export interface EpisodeFormData {
    episode_identifier?: string;
    episode_sort_order?: number;
    title: string;
    description: string;
    air_date: string;
    runtime: number | null;
    still_url?: string | null;
  }

  interface Props {
    episode?: TvEpisode | null;
    searchResults?: EpisodeDetailResult[];
    createMode?: boolean;
    onSave: (data: EpisodeFormData, stillFile?: File) => Promise<void>;
    onCancel: () => void;
    onDelete?: () => void;
    saving?: boolean;
    deleting?: boolean;
    hideFooter?: boolean;
    requestSave?: () => void;
    canSave?: boolean;
  }

  let {
    episode = null,
    searchResults = undefined,
    createMode = false,
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  let episode_identifier = $state('');
  let episode_sort_order = $state<number>(0);
  let title = $state('');
  let description = $state('');
  let air_date = $state('');
  let runtime = $state<number | null>(null);

  // Build source values from search results
  const titleSources = $derived(buildVideoSourceValues('title', searchResults as any[]));
  const descriptionSources = $derived(
    buildVideoSourceValues('description', searchResults as any[])
  );
  const airDateSources = $derived(buildVideoSourceValues('air_date', searchResults as any[]));
  const runtimeSources = $derived(
    buildVideoSourceValues('runtime', searchResults as any[], (v) => String(v))
  );
  const stillOptions = $derived(buildVideoCoverOptions(searchResults as any[], 'still_url' as any));

  // Source tracking
  let titleSource = $state<string | undefined>(undefined);
  let descriptionSource = $state<string | undefined>(undefined);
  let airDateSource = $state<string | undefined>(undefined);
  let runtimeSource = $state<string | undefined>(undefined);

  // Cover state
  let selectedStillUrl = $state<string | null>(null);
  let stillFile = $state<File | undefined>(undefined);

  // Initialize form from episode prop or search results
  $effect(() => {
    if (episode) {
      episode_identifier = episode.episode_identifier || '';
      episode_sort_order = episode.episode_sort_order ?? 0;
      title = episode.title || '';
      description = episode.description || '';
      air_date = episode.air_date || '';
      runtime = episode.runtime ?? null;
      selectedStillUrl = episode.still_url || null;
    } else if (searchResults && searchResults.length > 0) {
      title = getVideoInitialByPriority('title', searchResults as any[]);
      titleSource = getVideoInitialSourceForField('title', searchResults as any[]);
      description = getVideoInitialByPriority('description', searchResults as any[]);
      descriptionSource = getVideoInitialSourceForField('description', searchResults as any[]);
      air_date = getVideoInitialByPriority('air_date', searchResults as any[]);
      airDateSource = getVideoInitialSourceForField('air_date', searchResults as any[]);
      const rt = getVideoInitialByPriority('runtime', searchResults as any[]);
      runtime = rt ? parseInt(rt, 10) : null;
      runtimeSource = getVideoInitialSourceForField('runtime', searchResults as any[]);
      selectedStillUrl = stillOptions[0]?.url || null;
    } else {
      episode_identifier = '';
      episode_sort_order = 0;
      title = '';
      description = '';
      air_date = '';
      runtime = null;
      selectedStillUrl = null;
    }
  });

  // Expose canSave state
  $effect(() => {
    canSave = createMode ? episode_identifier.trim().length > 0 : title.trim().length > 0;
  });

  async function handleSave() {
    if (!canSave) return;

    const formData: EpisodeFormData = {
      title: title.trim(),
      description: description.trim(),
      air_date: air_date.trim(),
      runtime: runtime ?? null,
      ...(createMode && {
        episode_identifier: episode_identifier.trim(),
        episode_sort_order
      })
    };

    // Include image URL in metadata so the backend can lazy-cache it
    if (!stillFile && selectedStillUrl && selectedStillUrl !== episode?.still_url) {
      formData.still_url = selectedStillUrl;
    }

    await onSave(formData, stillFile || undefined);
  }

  // Expose save handler
  $effect(() => {
    requestSave = handleSave;
  });
</script>

<div class="episode-form">
  <div class="form-grid">
    {#if createMode}
      <div class="form-field">
        <label for="episode_identifier">Episode Number*</label>
        <input
          id="episode_identifier"
          type="text"
          bind:value={episode_identifier}
          placeholder="e.g. 1, 2, 3"
        />
      </div>

      <div class="form-field">
        <label for="episode_sort_order">Sort Order</label>
        <input id="episode_sort_order" type="number" bind:value={episode_sort_order} />
      </div>
    {/if}

    <div class="form-field full-width">
      <SourceField
        label={createMode ? 'Title' : 'Title*'}
        value={title}
        sourceValues={titleSources}
        currentSource={titleSource}
        onchange={(val, src) => {
          title = val;
          titleSource = src;
        }}
        type="text"
        placeholder="Enter episode title"
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
      <SourceField
        label="Runtime (minutes)"
        value={runtime !== null ? String(runtime) : ''}
        sourceValues={runtimeSources}
        currentSource={runtimeSource}
        onchange={(val, src) => {
          runtime = val ? parseInt(val, 10) : null;
          runtimeSource = src;
        }}
        type="number"
        placeholder="e.g. 45"
      />
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
        id="episode-still"
        label="Still"
        currentUrl={episode?.still_url}
        searchCovers={stillOptions}
        selectedUrl={selectedStillUrl}
        onSelect={(url, file) => {
          selectedStillUrl = url;
          stillFile = file;
        }}
        aspectRatio="landscape"
      />
    </div>
  </div>

  {#if !hideFooter}
    <div class="form-footer">
      <div class="footer-left">
        {#if episode && onDelete}
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
  .episode-form {
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
