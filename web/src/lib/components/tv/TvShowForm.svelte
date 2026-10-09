<script lang="ts">
  import { untrack } from 'svelte';
  import { onMount } from 'svelte';
  import type { TvShow, TvSearchResult, MovieSearchResult } from '$lib/types';
  import type { CastInput, CrewInput, KobloadQualityProfile } from '$lib/api/client';
  import { getKobloadQualityProfiles, listMediaRoots } from '$lib/api/client';
  import type { MediaRoot } from '$lib/types';
  import SourceField from '$lib/components/SourceField.svelte';
  import CoverSelector from '$lib/components/CoverSelector.svelte';
  import CastCrewEditor, {
    type CastEntry,
    type CrewEntry
  } from '$lib/components/shared/CastCrewEditor.svelte';
  import {
    buildVideoSourceValues,
    getVideoInitialByPriority,
    getVideoInitialSourceForField,
    buildVideoIdentifiers,
    buildVideoCoverOptions
  } from '$lib/components/video/video-form-utils';
  import { SearchSelect } from '@koblibs/ui';

  export interface TvShowFormData {
    title: string;
    sort_title?: string | null;
    original_title?: string | null;
    year: number | null;
    status: string;
    network: string;
    description: string;
    genres: string[];
    content_rating: string;
    identifiers: Record<string, string>;
    quality_profile_id?: string | null;
    preferred_languages?: string[] | null;
    media_root_id?: string | null;
    poster_url?: string | null;
    backdrop_url?: string | null;
    logo_url?: string | null;
    cast?: CastInput[];
    crew?: CrewInput[];
  }

  interface Props {
    show?: TvShow | null;
    searchResults?: (TvSearchResult | MovieSearchResult)[];
    onSave: (
      data: TvShowFormData,
      posterFile?: File,
      backdropFile?: File,
      logoFile?: File
    ) => Promise<void>;
    onCancel: () => void;
    onDelete?: () => void;
    saving?: boolean;
    deleting?: boolean;
    fixedFooter?: boolean;
    hideFooter?: boolean;
    hideCastCrew?: boolean;
    mediaType?: string;
    requestSave?: () => void;
    canSave?: boolean;
  }

  let {
    show = null,
    searchResults = [],
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    fixedFooter = false,
    hideFooter = false,
    hideCastCrew = false,
    mediaType = 'tv',
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  // Form state
  let title = $state('');
  let titleSource = $state<string | undefined>(undefined);
  let description = $state('');
  let descriptionSource = $state<string | undefined>(undefined);
  let year = $state<number | null>(null);
  let yearSource = $state<string | undefined>(undefined);
  let status = $state('');
  let statusSource = $state<string | undefined>(undefined);
  let network = $state('');
  let networkSource = $state<string | undefined>(undefined);
  let content_rating = $state('');
  let contentRatingSource = $state<string | undefined>(undefined);
  let sort_title = $state('');
  let original_title = $state('');
  let genresText = $state('');
  let quality_profile_id = $state<string | null>(null);
  let preferred_languages = $state('');
  let media_root_id = $state<string | null>(null);
  let identifiers = $state<Array<{ key: string; value: string }>>([]);
  let castEntries = $state<CastEntry[]>([]);
  let crewEntries = $state<CrewEntry[]>([]);
  let qualityProfiles = $state<KobloadQualityProfile[]>([]);
  let mediaRoots = $state<MediaRoot[]>([]);

  // Cover state
  let selectedPosterUrl = $state<string | null>(null);
  let posterFile = $state<File | undefined>(undefined);
  let selectedBackdropUrl = $state<string | null>(null);
  let backdropFile = $state<File | undefined>(undefined);
  let selectedLogoUrl = $state<string | null>(null);
  let logoFile = $state<File | undefined>(undefined);

  // Derived source values from search results
  const titleSources = $derived(buildVideoSourceValues('title', searchResults));
  const descriptionSources = $derived(buildVideoSourceValues('description', searchResults));
  const yearSources = $derived(buildVideoSourceValues('year', searchResults));
  const statusSources = $derived(buildVideoSourceValues('status', searchResults));
  const networkSources = $derived(buildVideoSourceValues('network', searchResults));
  const contentRatingSources = $derived(buildVideoSourceValues('content_rating', searchResults));
  const posterOptions = $derived(buildVideoCoverOptions(searchResults, 'poster_url'));
  const backdropOptions = $derived(buildVideoCoverOptions(searchResults, 'backdrop_url'));
  const logoCoverOptions = $derived(buildVideoCoverOptions(searchResults, 'logo_url'));

  onMount(async () => {
    try {
      const all = await getKobloadQualityProfiles();
      qualityProfiles = all.filter((p) => p.media_kind === 'tv');
    } catch {
      // Kobload not configured
    }
    try {
      const roots = await listMediaRoots();
      mediaRoots = roots.filter((r) => r.media_type === mediaType && r.enabled);
    } catch {
      // Media roots not configured
    }
  });

  // Initialize form from show or search results
  function initializeForm() {
    if (show) {
      // Edit mode - load from existing show
      title = show.title || '';
      description = show.description || '';
      year = show.year ?? null;
      status = show.status || '';
      network = show.network || '';
      content_rating = show.content_rating || '';
      sort_title = show.sort_title || '';
      original_title = show.original_title || '';
      genresText = show.genres?.join(', ') || '';
      quality_profile_id = show.quality_profile_id ?? null;
      preferred_languages = show.preferred_languages?.join(', ') ?? '';
      media_root_id = show.media_root_id ?? null;
      identifiers = show.identifiers
        ? Object.entries(show.identifiers).map(([key, value]) => ({ key, value: String(value) }))
        : [];
      castEntries = (show.cast ?? []).map((c) => ({
        person_id: c.person_id,
        person_name: c.person_name,
        person_image_url: c.person_image_url,
        character_name: c.character_name ?? ''
      }));
      crewEntries = (show.crew ?? []).map((c) => ({
        person_id: c.person_id,
        person_name: c.person_name,
        department: c.department,
        job: c.job
      }));
      selectedPosterUrl = show.poster_url || null;
      selectedBackdropUrl = show.backdrop_url || null;
      selectedLogoUrl = show.logo_url || null;
      titleSource = undefined;
      descriptionSource = undefined;
      yearSource = undefined;
      statusSource = undefined;
      networkSource = undefined;
      contentRatingSource = undefined;
    } else if (searchResults.length > 0) {
      // Create mode - initialize from search results
      title = getVideoInitialByPriority('title', searchResults);
      titleSource = getVideoInitialSourceForField('title', searchResults);
      description = getVideoInitialByPriority('description', searchResults);
      descriptionSource = getVideoInitialSourceForField('description', searchResults);
      const yearVal = getVideoInitialByPriority('year', searchResults);
      year = yearVal ? parseInt(yearVal, 10) : null;
      yearSource = getVideoInitialSourceForField('year', searchResults);
      status = getVideoInitialByPriority('status', searchResults);
      statusSource = getVideoInitialSourceForField('status', searchResults);
      network = getVideoInitialByPriority('network', searchResults);
      networkSource = getVideoInitialSourceForField('network', searchResults);
      content_rating = getVideoInitialByPriority('content_rating', searchResults);
      contentRatingSource = getVideoInitialSourceForField('content_rating', searchResults);

      // Auto-derive sort_title
      sort_title = deriveSortTitle(title);
      original_title = '';
      genresText = '';
      quality_profile_id = null;
      preferred_languages = '';
      identifiers = buildVideoIdentifiers(searchResults);
      castEntries = [];
      crewEntries = [];

      // Select first poster/backdrop from search results
      selectedPosterUrl = posterOptions[0]?.url || null;
      selectedBackdropUrl = backdropOptions[0]?.url || null;
      selectedLogoUrl = logoCoverOptions[0]?.url || null;
    } else {
      // Create mode with no search results
      title = '';
      description = '';
      year = null;
      status = '';
      network = '';
      content_rating = '';
      sort_title = '';
      original_title = '';
      genresText = '';
      quality_profile_id = null;
      preferred_languages = '';
      identifiers = [];
      castEntries = [];
      crewEntries = [];
      selectedPosterUrl = null;
      selectedBackdropUrl = null;
      selectedLogoUrl = null;
      titleSource = undefined;
      descriptionSource = undefined;
      yearSource = undefined;
      statusSource = undefined;
      networkSource = undefined;
      contentRatingSource = undefined;
    }
  }

  // Initialize once on mount
  $effect(() => {
    untrack(() => {
      initializeForm();
    });
  });

  // Re-initialize form state when searchResults change (e.g. user picks a different metadata match)
  let prevSearchResultsRef: (TvSearchResult | MovieSearchResult)[] | null = null;
  $effect(() => {
    const current = searchResults;
    if (prevSearchResultsRef !== null && current !== prevSearchResultsRef && current.length > 0) {
      untrack(() => {
        title = getVideoInitialByPriority('title', current) || title;
        sort_title = deriveSortTitle(title);
        description = getVideoInitialByPriority('description', current) || description;
        const yearVal = getVideoInitialByPriority('year', current);
        year = yearVal ? parseInt(yearVal, 10) : year;
        status = getVideoInitialByPriority('status', current) || status;
        network = getVideoInitialByPriority('network', current) || network;
        content_rating = getVideoInitialByPriority('content_rating', current) || content_rating;
        identifiers = buildVideoIdentifiers(current, show?.identifiers, true);

        // Update cover selections
        const newPoster = posterOptions[0]?.url;
        if (newPoster) selectedPosterUrl = newPoster;
        const newBackdrop = backdropOptions[0]?.url;
        if (newBackdrop) selectedBackdropUrl = newBackdrop;
        const newLogo = logoCoverOptions[0]?.url;
        if (newLogo) selectedLogoUrl = newLogo;
        posterFile = undefined;
        backdropFile = undefined;
        logoFile = undefined;

        // Update source indicators
        titleSource =
          titleSources.length > 0 ? getVideoInitialSourceForField('title', current) : undefined;
        descriptionSource =
          descriptionSources.length > 0
            ? getVideoInitialSourceForField('description', current)
            : undefined;
        yearSource =
          yearSources.length > 0 ? getVideoInitialSourceForField('year', current) : undefined;
        statusSource =
          statusSources.length > 0 ? getVideoInitialSourceForField('status', current) : undefined;
        networkSource =
          networkSources.length > 0 ? getVideoInitialSourceForField('network', current) : undefined;
        contentRatingSource =
          contentRatingSources.length > 0
            ? getVideoInitialSourceForField('content_rating', current)
            : undefined;
      });
    }
    prevSearchResultsRef = current;
  });

  $effect(() => {
    canSave = title.trim().length > 0;
  });

  function deriveSortTitle(t: string): string {
    const lower = t.toLowerCase();
    if (lower.startsWith('the ')) return t.substring(4) + ', The';
    if (lower.startsWith('a ')) return t.substring(2) + ', A';
    if (lower.startsWith('an ')) return t.substring(3) + ', An';
    return t;
  }

  async function handleSave() {
    if (!canSave) return;

    const genres = genresText
      .split(',')
      .map((g) => g.trim())
      .filter((g) => g.length > 0);

    const identifiersObj = identifiers.reduce(
      (acc, { key, value }) => {
        if (key.trim() && value.trim()) {
          acc[key.trim()] = value.trim();
        }
        return acc;
      },
      {} as Record<string, string>
    );

    const castInputs: CastInput[] = castEntries
      .filter((c) => c.person_id)
      .map((c, i) => ({
        person_id: c.person_id,
        character_name: c.character_name || null,
        cast_order: i
      }));

    const crewInputs: CrewInput[] = crewEntries
      .filter((c) => c.person_id && c.department && c.job)
      .map((c) => ({
        person_id: c.person_id,
        department: c.department,
        job: c.job
      }));

    const formData: TvShowFormData = {
      title: title.trim(),
      sort_title: sort_title.trim() || null,
      original_title: original_title.trim() || null,
      year: year ?? null,
      status: status.trim(),
      network: network.trim(),
      content_rating: content_rating.trim(),
      description: description.trim(),
      genres,
      identifiers: identifiersObj,
      quality_profile_id: quality_profile_id || null,
      preferred_languages: preferred_languages.trim()
        ? preferred_languages
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
        : null,
      media_root_id: media_root_id || null,
      cast: castInputs.length > 0 ? castInputs : undefined,
      crew: crewInputs.length > 0 ? crewInputs : undefined
    };

    // Include image URLs in metadata so the backend can lazy-cache them
    // (fetching external URLs client-side fails due to CORS)
    if (!posterFile && selectedPosterUrl && selectedPosterUrl !== show?.poster_url) {
      formData.poster_url = selectedPosterUrl;
    }
    if (!backdropFile && selectedBackdropUrl && selectedBackdropUrl !== show?.backdrop_url) {
      formData.backdrop_url = selectedBackdropUrl;
    }
    if (!logoFile && selectedLogoUrl && selectedLogoUrl !== show?.logo_url) {
      formData.logo_url = selectedLogoUrl;
    }

    await onSave(
      formData,
      posterFile || undefined,
      backdropFile || undefined,
      logoFile || undefined
    );
  }

  $effect(() => {
    requestSave = handleSave;
  });

  function addIdentifier() {
    identifiers = [...identifiers, { key: '', value: '' }];
  }

  function removeIdentifier(index: number) {
    identifiers = identifiers.filter((_, i) => i !== index);
  }
</script>

<div class="show-form">
  <div class="form-content">
    <div class="form-inner">
      {#if show && !hideFooter}
        <h2 class="form-title">Edit TV Show</h2>
      {/if}
      <div class="form-grid">
        <div class="form-field full-width">
          <SourceField
            label="Title"
            value={title}
            sourceValues={titleSources}
            currentSource={titleSource}
            onchange={(val, src) => {
              title = val;
              titleSource = src;
              if (!sort_title || sort_title === deriveSortTitle(title)) {
                sort_title = deriveSortTitle(val);
              }
            }}
            type="text"
            placeholder="Enter show title"
          />
        </div>

        <div class="form-field">
          <label for="sort_title">Sort Title</label>
          <input
            id="sort_title"
            type="text"
            bind:value={sort_title}
            placeholder="e.g. Office, The"
          />
        </div>

        <div class="form-field">
          <label for="original_title">Original Title</label>
          <input
            id="original_title"
            type="text"
            bind:value={original_title}
            placeholder="Original language title"
          />
        </div>

        <div class="form-field">
          <SourceField
            label="Year"
            value={year !== null ? String(year) : ''}
            sourceValues={yearSources}
            currentSource={yearSource}
            onchange={(val, src) => {
              year = val ? parseInt(val, 10) : null;
              yearSource = src;
            }}
            type="number"
            placeholder="e.g. 2024"
          />
        </div>

        <div class="form-field">
          <SourceField
            label="Status"
            value={status}
            sourceValues={statusSources}
            currentSource={statusSource}
            onchange={(val, src) => {
              status = val;
              statusSource = src;
            }}
            type="text"
            placeholder="e.g. Continuing, Ended"
          />
        </div>

        <div class="form-field">
          <SourceField
            label="Network"
            value={network}
            sourceValues={networkSources}
            currentSource={networkSource}
            onchange={(val, src) => {
              network = val;
              networkSource = src;
            }}
            type="text"
            placeholder="e.g. HBO, Netflix"
          />
        </div>

        <div class="form-field">
          <SourceField
            label="Content Rating"
            value={content_rating}
            sourceValues={contentRatingSources}
            currentSource={contentRatingSource}
            onchange={(val, src) => {
              content_rating = val;
              contentRatingSource = src;
            }}
            type="text"
            placeholder="e.g. TV-MA"
          />
        </div>

        {#if qualityProfiles.length > 0}
          <div class="form-field">
            <label for="quality_profile">Quality Profile</label>
            <SearchSelect
              id="quality_profile"
              items={[
                { value: '', label: 'Default' },
                ...qualityProfiles.map((p) => ({
                  value: p.id,
                  label: `${p.name}${p.is_default ? ' (default)' : ''}`
                }))
              ]}
              value={quality_profile_id ?? ''}
              onchange={(v) => {
                quality_profile_id = v || null;
              }}
            />
          </div>
        {/if}

        <div class="form-field">
          <label for="preferred-languages">Content Language</label>
          <input
            id="preferred-languages"
            type="text"
            bind:value={preferred_languages}
            placeholder="Leave blank for profile default, e.g. japanese"
          />
          <span class="field-hint"
            >Override the quality profile's language filter for this show. Comma-separated.</span
          >
        </div>

        {#if mediaRoots.length > 0}
          <div class="form-field">
            <label for="media-root">Media Root</label>
            <SearchSelect
              id="media-root"
              items={[
                { value: '', label: 'Auto (default)' },
                ...mediaRoots.map((r) => ({
                  value: r.id,
                  label: `${r.name || r.path}${r.is_default ? ' (default)' : ''}`
                }))
              ]}
              value={media_root_id ?? ''}
              onchange={(v) => {
                media_root_id = v || null;
              }}
            />
          </div>
        {/if}

        <div class="form-field full-width">
          <label for="genres">Genres</label>
          <input
            id="genres"
            type="text"
            bind:value={genresText}
            placeholder="Comma-separated, e.g. Drama, Thriller"
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
            id="poster"
            label="Poster"
            currentUrl={show?.poster_url}
            searchCovers={posterOptions}
            selectedUrl={selectedPosterUrl}
            onSelect={(url, file) => {
              selectedPosterUrl = url;
              posterFile = file;
            }}
            aspectRatio="portrait"
          />
        </div>

        <div class="form-field full-width">
          <CoverSelector
            id="backdrop"
            label="Backdrop"
            currentUrl={show?.backdrop_url}
            searchCovers={backdropOptions}
            selectedUrl={selectedBackdropUrl}
            onSelect={(url, file) => {
              selectedBackdropUrl = url;
              backdropFile = file;
            }}
            aspectRatio="landscape"
          />
        </div>

        <div class="form-field full-width">
          <CoverSelector
            id="logo"
            label="Logo"
            currentUrl={show?.logo_url}
            searchCovers={logoCoverOptions}
            selectedUrl={selectedLogoUrl}
            onSelect={(url, file) => {
              selectedLogoUrl = url;
              logoFile = file;
            }}
            aspectRatio="landscape"
          />
        </div>

        <div class="form-field full-width">
          <div class="identifiers-header">
            <span>Identifiers</span>
            <button type="button" class="btn-sec small" onclick={addIdentifier}>Add</button>
          </div>
          <div class="identifiers-list">
            {#each identifiers as identifier, i (i)}
              <div class="identifier-row">
                <input
                  type="text"
                  bind:value={identifier.key}
                  placeholder="Key (e.g. tvdb_id)"
                  class="identifier-key"
                />
                <input
                  type="text"
                  bind:value={identifier.value}
                  placeholder="Value"
                  class="identifier-value"
                />
                <button
                  type="button"
                  class="btn-sec small"
                  onclick={() => removeIdentifier(i)}
                  aria-label="Remove identifier"
                >
                  &times;
                </button>
              </div>
            {/each}
          </div>
        </div>

        {#if !hideCastCrew}
          <div class="form-field full-width">
            <CastCrewEditor bind:cast={castEntries} bind:crew={crewEntries} />
          </div>
        {/if}
      </div>
    </div>
  </div>

  {#if !hideFooter}
    <div class="form-footer" class:fixed-footer={fixedFooter}>
      <div class={fixedFooter ? 'fixed-footer-inner' : 'footer-content'}>
        {#if show && onDelete}
          <button
            type="button"
            class="btn btn-danger"
            onclick={onDelete}
            disabled={deleting || saving}
          >
            <span class="material-icons-outlined">delete</span>
            {deleting ? 'Deleting...' : 'Delete'}
          </button>
        {/if}
        <div class="spacer"></div>
        <button type="button" class="btn btn-sec" onclick={onCancel} disabled={saving || deleting}>
          Cancel
        </button>
        <button
          type="button"
          class="btn btn-primary"
          onclick={handleSave}
          disabled={!canSave || saving || deleting}
        >
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .show-form {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .form-content {
    flex: 1;
    overflow-y: auto;
    padding: 0;
    width: 100%;
  }

  .form-inner {
    max-width: 1000px;
    margin: 0 auto;
    padding: 40px 20px;
  }

  .form-title {
    margin: 0 0 32px;
    font-size: 24px;
    font-weight: 800;
    color: var(--ink);
    letter-spacing: -0.02em;
  }

  .form-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 24px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 8px;
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

  .identifiers-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .identifiers-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .identifier-row {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: 8px;
    align-items: center;
  }

  .identifier-key,
  .identifier-value {
    min-width: 0;
  }

  button.small {
    padding: 6px 12px;
    font-size: 13px;
  }

  .form-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 24px 0;
    border-top: 1px solid var(--ring);
    margin-top: 24px;
    flex-shrink: 0;
  }

  .form-footer.fixed-footer {
    position: sticky;
    bottom: 0;
    left: 0;
    right: 0;
    width: 100%;
    padding: 16px 40px;
    background: linear-gradient(180deg, rgba(17, 20, 25, 0.85), rgba(21, 24, 30, 0.95));
    backdrop-filter: blur(20px) saturate(1.8);
    -webkit-backdrop-filter: blur(20px) saturate(1.8);
    border-top: 1px solid var(--ring);
    z-index: 1001;
    display: flex;
    align-items: center;
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .form-footer.fixed-footer {
    background: linear-gradient(180deg, rgba(239, 230, 219, 0.85), rgba(245, 239, 231, 0.95));
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.05);
  }

  .fixed-footer-inner {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .footer-content {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .spacer {
    flex: 1;
  }
</style>
