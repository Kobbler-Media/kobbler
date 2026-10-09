<script lang="ts">
  import { untrack } from 'svelte';
  import type { Movie, MovieSearchResult, TvSearchResult } from '$lib/types';
  import {
    getKobloadQualityProfiles,
    listMediaRoots,
    type KobloadQualityProfile,
    type CastInput,
    type CrewInput
  } from '$lib/api/client';
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
  export interface MovieFormData {
    title: string;
    sort_title?: string | null;
    original_title?: string | null;
    release_date?: string | null;
    year?: number | null;
    description?: string | null;
    runtime?: number | null;
    content_rating?: string | null;
    genres?: string[];
    identifiers?: Record<string, string>;
    rating?: number | null;
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
    movie?: Movie | null;
    searchResults?: (MovieSearchResult | TvSearchResult)[];
    onSave: (
      data: MovieFormData,
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
    requestSave?: () => void;
    canSave?: boolean;
  }

  let {
    movie = null,
    searchResults = [],
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    fixedFooter = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  // Helper to derive sort title
  function deriveSortTitle(t: string): string {
    const prefixes = ['The ', 'A ', 'An '];
    for (const prefix of prefixes) {
      if (t.startsWith(prefix)) {
        return t.substring(prefix.length) + ', ' + prefix.trim();
      }
    }
    return t;
  }

  // Build source values from search results
  const titleSources = $derived(buildVideoSourceValues('title', searchResults));
  const descriptionSources = $derived(buildVideoSourceValues('description', searchResults));
  const yearSources = $derived(buildVideoSourceValues('year', searchResults, (v) => String(v)));
  const ratingSources = $derived(buildVideoSourceValues('rating', searchResults, (v) => String(v)));
  const contentRatingSources = $derived(buildVideoSourceValues('content_rating', searchResults));

  // Initial values from movie or search results
  const initTitle = $derived(movie?.title || getVideoInitialByPriority('title', searchResults));
  const initDescription = $derived(
    movie?.description || getVideoInitialByPriority('description', searchResults)
  );
  const initYear = $derived.by(() => {
    if (movie?.year) return movie.year;
    const yr = getVideoInitialByPriority('year', searchResults);
    return yr ? Number(yr) : null;
  });
  const initRating = $derived.by(() => {
    if (movie?.rating !== null && movie?.rating !== undefined) return movie.rating;
    const r = getVideoInitialByPriority('rating', searchResults);
    return r ? parseFloat(r) : null;
  });
  const initContentRating = $derived(
    movie?.content_rating || getVideoInitialByPriority('content_rating', searchResults)
  );
  const initIdentifiers = $derived.by(() =>
    buildVideoIdentifiers(searchResults, movie?.identifiers, searchResults.length > 0)
  );
  const initPosterUrl = $derived(
    movie?.poster_url || getVideoInitialByPriority('poster_url', searchResults)
  );
  const initBackdropUrl = $derived(
    movie?.backdrop_url || getVideoInitialByPriority('backdrop_url', searchResults)
  );
  const initLogoUrl = $derived(
    movie?.logo_url || getVideoInitialByPriority('logo_url', searchResults)
  );

  // Cover options from search results
  const posterOptions = $derived(buildVideoCoverOptions(searchResults, 'poster_url'));
  const backdropOptions = $derived(buildVideoCoverOptions(searchResults, 'backdrop_url'));
  const logoOptions = $derived(buildVideoCoverOptions(searchResults, 'logo_url'));

  // Form state
  let title = $state(untrack(() => initTitle) || '');
  let titleSource = $state<string | undefined>(
    untrack(() =>
      titleSources.length > 0 ? getVideoInitialSourceForField('title', searchResults) : undefined
    )
  );
  let sortTitle = $state(
    untrack(() => {
      const t = movie?.sort_title || initTitle;
      return t ? deriveSortTitle(t) : '';
    })
  );
  let _sortTitleManuallyEdited = $state(false);
  let originalTitle = $state(untrack(() => movie?.original_title) || '');
  let releaseDate = $state('');
  let year = $state<number | null>(untrack(() => initYear));
  let yearSource = $state<string | undefined>(
    untrack(() =>
      yearSources.length > 0 ? getVideoInitialSourceForField('year', searchResults) : undefined
    )
  );
  let runtime = $state<number | null>(untrack(() => movie?.runtime) ?? null);
  let contentRating = $state(untrack(() => initContentRating) || '');
  let contentRatingSource = $state<string | undefined>(
    untrack(() =>
      contentRatingSources.length > 0
        ? getVideoInitialSourceForField('content_rating', searchResults)
        : undefined
    )
  );
  let description = $state(untrack(() => initDescription) || '');
  let descriptionSource = $state<string | undefined>(
    untrack(() =>
      descriptionSources.length > 0
        ? getVideoInitialSourceForField('description', searchResults)
        : undefined
    )
  );
  let genresText = $state(untrack(() => movie?.genres?.join(', ')) || '');
  let rating = $state<number | null>(untrack(() => initRating));
  let ratingSource = $state<string | undefined>(
    untrack(() =>
      ratingSources.length > 0 ? getVideoInitialSourceForField('rating', searchResults) : undefined
    )
  );
  let qualityProfileId = $state<string | null>(untrack(() => movie?.quality_profile_id) ?? null);
  let preferredLanguages = $state<string>(
    untrack(() => movie?.preferred_languages?.join(', ') ?? '')
  );
  let qualityProfiles = $state<KobloadQualityProfile[]>([]);
  let mediaRootId = $state<string | null>(untrack(() => movie?.media_root_id) ?? null);
  let mediaRoots = $state<MediaRoot[]>([]);
  let identifiers = $state<Array<{ key: string; value: string }>>(untrack(() => initIdentifiers));
  let castEntries = $state<CastEntry[]>(
    untrack(() =>
      (movie?.cast ?? []).map((c) => ({
        person_id: c.person_id,
        person_name: c.person_name,
        person_image_url: c.person_image_url,
        character_name: c.character_name ?? ''
      }))
    )
  );
  let crewEntries = $state<CrewEntry[]>(
    untrack(() =>
      (movie?.crew ?? []).map((c) => ({
        person_id: c.person_id,
        person_name: c.person_name,
        department: c.department,
        job: c.job
      }))
    )
  );
  let selectedPosterUrl = $state<string | null>(untrack(() => initPosterUrl) || null);
  let posterFile = $state<File | null>(null);
  let selectedBackdropUrl = $state<string | null>(untrack(() => initBackdropUrl) || null);
  let backdropFile = $state<File | null>(null);
  let selectedLogoUrl = $state<string | null>(untrack(() => initLogoUrl) || null);
  let logoFile = $state<File | null>(null);

  // Load quality profiles and media roots on mount
  $effect(() => {
    (async () => {
      try {
        const all = await getKobloadQualityProfiles();
        qualityProfiles = all.filter((p) => p.media_kind === 'movie');
      } catch {
        // Kobload not configured
      }
      try {
        const roots = await listMediaRoots();
        mediaRoots = roots.filter((r) => r.media_type === 'movie' && r.enabled);
      } catch {
        // Media roots not configured
      }
    })();
  });

  // Re-initialize form state when searchResults change
  let prevSearchResultsRef: (MovieSearchResult | TvSearchResult)[] | null = null;
  $effect(() => {
    const current = searchResults;
    if (prevSearchResultsRef !== null && current !== prevSearchResultsRef && current.length > 0) {
      untrack(() => {
        title = initTitle;
        sortTitle = deriveSortTitle(initTitle);
        _sortTitleManuallyEdited = false;
        description = initDescription;
        year = initYear;
        rating = initRating;
        contentRating = initContentRating;
        identifiers = initIdentifiers;
        selectedPosterUrl = initPosterUrl || null;
        selectedBackdropUrl = initBackdropUrl || null;
        selectedLogoUrl = initLogoUrl || null;
        posterFile = null;
        backdropFile = null;
        logoFile = null;

        // Update source indicators
        titleSource =
          titleSources.length > 0
            ? getVideoInitialSourceForField('title', searchResults)
            : undefined;
        descriptionSource =
          descriptionSources.length > 0
            ? getVideoInitialSourceForField('description', searchResults)
            : undefined;
        yearSource =
          yearSources.length > 0 ? getVideoInitialSourceForField('year', searchResults) : undefined;
        ratingSource =
          ratingSources.length > 0
            ? getVideoInitialSourceForField('rating', searchResults)
            : undefined;
        contentRatingSource =
          contentRatingSources.length > 0
            ? getVideoInitialSourceForField('content_rating', searchResults)
            : undefined;
      });
    }
    prevSearchResultsRef = current;
  });

  // Expose canSave state
  $effect(() => {
    canSave = title.trim().length > 0 && !saving;
  });

  function handlePosterSelect(url: string | null, file?: File) {
    selectedPosterUrl = url;
    posterFile = file || null;
  }

  function handleBackdropSelect(url: string | null, file?: File) {
    selectedBackdropUrl = url;
    backdropFile = file || null;
  }

  function handleLogoSelect(url: string | null, file?: File) {
    selectedLogoUrl = url;
    logoFile = file || null;
  }

  function addIdentifier() {
    identifiers = [...identifiers, { key: '', value: '' }];
  }

  function removeIdentifier(index: number) {
    identifiers = identifiers.filter((_, i) => i !== index);
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

    const formData: MovieFormData = {
      title: title.trim(),
      sort_title: sortTitle.trim() || null,
      original_title: originalTitle.trim() || null,
      release_date: releaseDate.trim() || null,
      year: year ?? null,
      runtime: runtime ?? null,
      content_rating: contentRating.trim() || null,
      description: description.trim() || null,
      genres: genres.length > 0 ? genres : undefined,
      identifiers: Object.keys(identifiersObj).length > 0 ? identifiersObj : undefined,
      rating: rating ?? null,
      quality_profile_id: qualityProfileId || null,
      preferred_languages: preferredLanguages.trim()
        ? preferredLanguages
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
        : null,
      media_root_id: mediaRootId || null,
      cast: castInputs.length > 0 ? castInputs : undefined,
      crew: crewInputs.length > 0 ? crewInputs : undefined
    };

    // Include image URLs in metadata so the backend can lazy-cache them
    // (fetching external URLs client-side fails due to CORS)
    if (!posterFile && selectedPosterUrl && selectedPosterUrl !== movie?.poster_url) {
      formData.poster_url = selectedPosterUrl;
    }
    if (!backdropFile && selectedBackdropUrl && selectedBackdropUrl !== movie?.backdrop_url) {
      formData.backdrop_url = selectedBackdropUrl;
    }
    if (!logoFile && selectedLogoUrl && selectedLogoUrl !== movie?.logo_url) {
      formData.logo_url = selectedLogoUrl;
    }

    await onSave(
      formData,
      posterFile || undefined,
      backdropFile || undefined,
      logoFile || undefined
    );
  }

  // Expose save handler
  $effect(() => {
    requestSave = handleSave;
  });
</script>

<div class="movie-form">
  <div class="form-content">
    <div class="form-inner">
      {#if movie && !hideFooter}
        <h2 class="form-title">Edit Movie</h2>
      {/if}

      <div class="form-sections">
        <div class="covers-section">
          <CoverSelector
            id="poster-selector"
            label="Poster"
            currentUrl={movie?.poster_url || null}
            searchCovers={posterOptions}
            selectedUrl={selectedPosterUrl}
            onSelect={handlePosterSelect}
          />
          <CoverSelector
            id="backdrop-selector"
            label="Backdrop"
            currentUrl={movie?.backdrop_url || null}
            searchCovers={backdropOptions}
            selectedUrl={selectedBackdropUrl}
            onSelect={handleBackdropSelect}
            aspectRatio="landscape"
          />
          <CoverSelector
            id="logo-selector"
            label="Logo"
            currentUrl={movie?.logo_url || null}
            searchCovers={logoOptions}
            selectedUrl={selectedLogoUrl}
            onSelect={handleLogoSelect}
            aspectRatio="landscape"
          />
        </div>

        <div class="form-grid">
          <div class="form-field full-width">
            <SourceField
              label="Title *"
              value={title}
              sourceValues={titleSources}
              currentSource={titleSource}
              onchange={(v, s) => {
                title = v;
                if (s) titleSource = s;
                if (!_sortTitleManuallyEdited) sortTitle = deriveSortTitle(v);
              }}
              placeholder="Movie title..."
            />
          </div>

          <div class="form-field full-width">
            <label for="sort-title">Sort Title</label>
            <input
              id="sort-title"
              type="text"
              value={sortTitle}
              oninput={(e) => {
                sortTitle = (e.target as HTMLInputElement).value;
                _sortTitleManuallyEdited = true;
              }}
              placeholder="e.g. Dark Knight, The"
            />
          </div>

          <div class="form-field full-width">
            <label for="original-title">Original Title</label>
            <input
              id="original-title"
              type="text"
              bind:value={originalTitle}
              placeholder="Original language title"
            />
          </div>

          <div class="form-field">
            <SourceField
              label="Year"
              value={year?.toString() || ''}
              sourceValues={yearSources}
              currentSource={yearSource}
              type="number"
              onchange={(v, s) => {
                year = v ? parseInt(v) : null;
                if (s) yearSource = s;
              }}
              placeholder="2024"
            />
          </div>

          <div class="form-field">
            <label for="release-date">Release Date</label>
            <input
              id="release-date"
              type="text"
              bind:value={releaseDate}
              placeholder="YYYY-MM-DD"
            />
          </div>

          <div class="form-field">
            <label for="runtime">Runtime (minutes)</label>
            <input id="runtime" type="number" bind:value={runtime} placeholder="120" />
          </div>

          <div class="form-field">
            <SourceField
              label="Content Rating"
              value={contentRating}
              sourceValues={contentRatingSources}
              currentSource={contentRatingSource}
              onchange={(v, s) => {
                contentRating = v;
                if (s) contentRatingSource = s;
              }}
              placeholder="PG-13, R"
            />
          </div>

          <div class="form-field">
            <SourceField
              label="Rating"
              value={rating?.toString() || ''}
              sourceValues={ratingSources}
              currentSource={ratingSource}
              type="number"
              onchange={(v, s) => {
                rating = v ? parseFloat(v) : null;
                if (s) ratingSource = s;
              }}
              placeholder="8.5"
            />
          </div>

          {#if qualityProfiles.length > 0}
            <div class="form-field">
              <label for="quality-profile">Quality Profile</label>
              <SearchSelect
                id="quality-profile"
                items={[
                  { value: '', label: 'Default' },
                  ...qualityProfiles.map((p) => ({
                    value: p.id,
                    label: `${p.name}${p.is_default ? ' (default)' : ''}`
                  }))
                ]}
                value={qualityProfileId ?? ''}
                onchange={(v) => {
                  qualityProfileId = v || null;
                }}
              />
            </div>
          {/if}

          <div class="form-field">
            <label for="preferred-languages">Content Language</label>
            <input
              id="preferred-languages"
              type="text"
              bind:value={preferredLanguages}
              placeholder="Leave blank for profile default, e.g. japanese"
            />
            <span class="field-hint"
              >Override the quality profile's language filter for this movie. Comma-separated.</span
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
                value={mediaRootId ?? ''}
                onchange={(v) => {
                  mediaRootId = v || null;
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
              placeholder="Comma-separated, e.g. Action, Thriller"
            />
          </div>

          <div class="form-field full-width">
            <SourceField
              label="Description"
              value={description}
              sourceValues={descriptionSources}
              currentSource={descriptionSource}
              type="textarea"
              onchange={(v, s) => {
                description = v;
                if (s) descriptionSource = s;
              }}
              placeholder="Movie description..."
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
                    placeholder="Key (e.g. tmdb_id)"
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
                    ×
                  </button>
                </div>
              {/each}
            </div>
          </div>

          <div class="form-field full-width">
            <CastCrewEditor bind:cast={castEntries} bind:crew={crewEntries} />
          </div>
        </div>
      </div>
    </div>
  </div>

  {#if !hideFooter}
    <div class="form-footer" class:fixed-footer={fixedFooter}>
      <div class={fixedFooter ? 'fixed-footer-inner' : 'footer-content'}>
        {#if movie && onDelete}
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
  .movie-form {
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

  .form-sections {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .covers-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
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
