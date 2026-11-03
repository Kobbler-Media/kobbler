<script lang="ts">
  /**
   * BookForm - Unified form for creating and editing books.
   * Supports:
   * - Create mode: source-aware fields from metadata search results
   * - Edit mode: editing existing books with link-source capability
   * - Ingest mode: editing ingest queue items
   */
  import { untrack } from 'svelte';
  import type { Book, AuthorInput, BookFormData, SeriesMembership } from '#lib/types.js';
  import type { MetadataResult } from '#lib/api/client.js';
  import type { ProviderInfo } from './MetadataSearchModal.svelte';
  import SourceField from './SourceField.svelte';
  import CoverSelector from './CoverSelector.svelte';
  import { MultiSearchSelect, SearchSelect } from '@koblibs/ui';
  import * as api from '#lib/api/client.js';
  import { getProviderCode, getProviderName } from '#lib/utils/format.js';

  interface Props {
    /** Existing book to edit (includes ingest items which are now Book-shaped). If undefined, form is in create mode. */
    book?: Book | null;
    /** Raw search results from metadata providers (for create mode source-aware fields) */
    searchResults?: MetadataResult[];
    /** Provider info with priorities from backend (for sorting results) */
    providers?: ProviderInfo[];
    onSave: (data: BookFormData, coverFile?: File) => Promise<void>;
    onCancel: () => void;
    onDelete?: () => void;
    saving?: boolean;
    deleting?: boolean;
    /** Error message to display at the top of the form. */
    error?: string;
    /** When true, footer becomes a fixed full-width bottom bar (for full-page edit views). */
    fixedFooter?: boolean;
    /** When true, BookForm does not render its own footer. Parent should render buttons externally (e.g. via ExpandableSidebar footer snippet) and bind requestSave/canSave. */
    hideFooter?: boolean;
    /** Bindable: call this to trigger the form's save logic. Only useful when hideFooter is true. */
    requestSave?: () => void;
    /** Bindable: whether the form can be saved (title is non-empty and not currently saving). */
    canSave?: boolean;
  }

  interface IdentifierEntry {
    key: string;
    value: string;
  }

  type SourceValue = {
    source: string;
    sourceName: string;
    value: string;
  };

  /** Extended source value for authors that includes hardcover IDs */
  type AuthorSourceValue = SourceValue & {
    authors: AuthorInput[];
  };

  /** Extended source value for series that includes hardcover ID */
  type SeriesSourceValue = SourceValue & {
    hardcover_id?: string;
  };

  let {
    book = null,
    searchResults = [],
    providers = [],
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    error,
    fixedFooter = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  // Get provider priority from backend data (lower = higher priority)
  function getProviderPriority(providerType: string): number {
    const provider = providers.find((p) => p.id === providerType);
    return provider?.priority ?? 999;
  }

  // Sort search results by provider priority from backend
  const sortedResults = $derived(
    [...searchResults].sort(
      (a, b) => getProviderPriority(a.provider) - getProviderPriority(b.provider)
    )
  );

  // Determine mode
  const isCreateMode = $derived(!book);
  const isIngestMode = $derived(!!book?.ingest_info);

  // Build source values for each field from all search results
  function buildSourceValues(
    field: keyof MetadataResult,
    transform?: (val: unknown) => string
  ): SourceValue[] {
    const values: SourceValue[] = [];
    for (const result of searchResults) {
      const rawVal = result[field];
      if (rawVal !== null && rawVal !== undefined && rawVal !== '') {
        const strVal = transform ? transform(rawVal) : String(rawVal);
        if (strVal) {
          values.push({
            source: getProviderCode(result.provider),
            sourceName: getProviderName(result.provider),
            value: strVal
          });
        }
      }
    }
    return values;
  }

  function buildAuthorsSourceValues(): AuthorSourceValue[] {
    const values: AuthorSourceValue[] = [];
    for (const result of searchResults) {
      if (result.authors && result.authors.length > 0) {
        // Build author inputs with hardcover IDs when available
        const authorInputs: AuthorInput[] = result.authors.map((name, idx) => {
          const withId = result.authors_with_ids?.[idx];
          return {
            name,
            hardcover_id: withId?.provider_id
          };
        });
        values.push({
          source: getProviderCode(result.provider),
          sourceName: getProviderName(result.provider),
          value: result.authors.join(', '),
          authors: authorInputs
        });
      }
    }
    return values;
  }

  function buildSeriesSourceValues(): SeriesSourceValue[] {
    const values: SeriesSourceValue[] = [];
    for (const result of searchResults) {
      if (result.series) {
        values.push({
          source: getProviderCode(result.provider),
          sourceName: getProviderName(result.provider),
          value: result.series,
          hardcover_id: result.series_provider_id ?? undefined
        });
      }
    }
    return values;
  }

  function buildSubjectsSourceValues(): SourceValue[] {
    const values: SourceValue[] = [];
    for (const result of searchResults) {
      if (result.subjects && result.subjects.length > 0) {
        values.push({
          source: getProviderCode(result.provider),
          sourceName: getProviderName(result.provider),
          value: result.subjects.join(', ')
        });
      }
    }
    return values;
  }

  // Pre-computed source values (only relevant in create mode with search results)
  const titleSources = $derived(buildSourceValues('title'));
  const descriptionSources = $derived(buildSourceValues('description'));
  const publisherSources = $derived(buildSourceValues('publisher'));
  const pubdateSources = $derived(
    buildSourceValues('publish_date', (v) => {
      const s = String(v);
      return s.length >= 4 ? s.substring(0, 4) : s;
    })
  );
  // Language code to name mapping
  const languageNames: Record<string, string> = {
    en: 'English',
    es: 'Spanish',
    fr: 'French',
    de: 'German',
    it: 'Italian',
    pt: 'Portuguese',
    ru: 'Russian',
    ja: 'Japanese',
    zh: 'Chinese',
    ko: 'Korean',
    ar: 'Arabic',
    nl: 'Dutch',
    sv: 'Swedish',
    no: 'Norwegian',
    da: 'Danish',
    fi: 'Finnish',
    pl: 'Polish',
    cs: 'Czech',
    hu: 'Hungarian',
    tr: 'Turkish',
    he: 'Hebrew',
    th: 'Thai',
    vi: 'Vietnamese',
    id: 'Indonesian',
    ms: 'Malay',
    hi: 'Hindi',
    bn: 'Bengali',
    uk: 'Ukrainian',
    el: 'Greek',
    ro: 'Romanian',
    bg: 'Bulgarian',
    hr: 'Croatian',
    sr: 'Serbian',
    sk: 'Slovak',
    sl: 'Slovenian',
    lt: 'Lithuanian',
    lv: 'Latvian',
    et: 'Estonian',
    // Common variants
    eng: 'English',
    spa: 'Spanish',
    fra: 'French',
    deu: 'German',
    ita: 'Italian',
    por: 'Portuguese',
    rus: 'Russian',
    jpn: 'Japanese',
    zho: 'Chinese',
    kor: 'Korean'
  };

  function getLanguageName(code: string): string {
    const lower = code.toLowerCase().trim();
    return languageNames[lower] || code;
  }

  const languageSources = $derived(
    buildSourceValues('language', (v) => {
      const code = String(v);
      return getLanguageName(code);
    })
  );
  // Filter out 0 or invalid page counts
  const pageCountSources = $derived(
    buildSourceValues('page_count', (v) => {
      const num = Number(v);
      return num > 0 ? String(num) : '';
    }).filter((s) => s.value !== '')
  );
  const ratingSources = $derived(buildSourceValues('rating', (v) => String(v)));
  const seriesSources = $derived(buildSeriesSourceValues());
  const seriesIndexSources = $derived(buildSourceValues('series_index', (v) => String(v)));
  const authorsSources = $derived(buildAuthorsSourceValues());
  const genresSources = $derived(buildSubjectsSourceValues());

  // Helper functions to get initial values - uses priority-based selection
  // Finds the highest-priority provider that has a non-empty value for the field
  function getInitialByPriority<T>(
    field: keyof MetadataResult,
    validator: (val: unknown) => val is T
  ): { value: T; source: string } | null {
    const sorted = sortedResults;
    for (const result of sorted) {
      const val = result[field];
      if (validator(val)) {
        const source = getProviderCode(result.provider);
        return { value: val, source };
      }
    }
    return null;
  }

  function isNonEmptyString(val: unknown): val is string {
    return typeof val === 'string' && val.trim() !== '';
  }

  function isNumber(val: unknown): val is number {
    return typeof val === 'number';
  }

  function isPositiveNumber(val: unknown): val is number {
    return typeof val === 'number' && val > 0;
  }

  function isNonEmptyArray(val: unknown): val is string[] {
    return Array.isArray(val) && val.length > 0;
  }

  function getInitialString(field: keyof MetadataResult): string {
    const result = getInitialByPriority(field, isNonEmptyString);
    return result?.value || '';
  }

  function getInitialNumber(field: keyof MetadataResult): number | null {
    const result = getInitialByPriority(field, isNumber);
    return result?.value ?? null;
  }

  function getInitialStringArray(field: keyof MetadataResult): string[] {
    const result = getInitialByPriority(field, isNonEmptyArray);
    return result?.value || [];
  }

  // Get the source code for the best provider that has data for a specific field
  function getInitialSourceForField(field: keyof MetadataResult): string | undefined {
    const sorted = sortedResults;
    for (const result of sorted) {
      const val = result[field];
      // Check if the field has a non-empty value
      if (
        val !== null &&
        val !== undefined &&
        val !== '' &&
        !(Array.isArray(val) && val.length === 0)
      ) {
        return getProviderCode(result.provider);
      }
    }
    return sorted.length > 0 ? getProviderCode(sorted[0].provider) : undefined;
  }

  // Initialize form values from book or searchResults
  // (ingest items are now Book-shaped, so book?.field covers both cases)
  const initTitle = $derived(book?.title || getInitialString('title'));
  const initAuthors = $derived.by((): AuthorInput[] => {
    if (book?.authors && book.authors.length > 0) {
      return book.authors.map((a) => ({ name: a.name }));
    }
    // From search results - try to get hardcover IDs
    const sorted = sortedResults;
    for (const result of sorted) {
      if (result.authors && result.authors.length > 0) {
        return result.authors.map((name, idx) => ({
          name,
          hardcover_id: result.authors_with_ids?.[idx]?.provider_id
        }));
      }
    }
    return [];
  });
  const initSeries = $derived(book?.series_name || getInitialString('series'));
  const initSeriesIndex = $derived(book?.series_index ?? getInitialNumber('series_index'));
  const initPublisher = $derived(book?.publisher || getInitialString('publisher'));
  const initPubdate = $derived.by(() => {
    if (book?.pubdate) return new Date(book.pubdate).getFullYear().toString();
    const pd = getInitialString('publish_date');
    return pd.length >= 4 ? pd.substring(0, 4) : pd;
  });
  const initDescription = $derived(book?.description || getInitialString('description'));
  const initLanguage = $derived.by(() => {
    const code = book?.language || getInitialString('language');
    return code ? getLanguageName(code) : '';
  });
  const initPageCount = $derived.by(() => {
    if (book?.page_count && book.page_count > 0) return book.page_count;
    // Use positive-only validator for page count from search results
    const result = getInitialByPriority('page_count', isPositiveNumber);
    return result?.value ?? null;
  });
  const initRating = $derived(book?.rating ?? getInitialNumber('rating'));
  const initGenres = $derived.by(() => {
    if (book?.genres) return book.genres.map((g) => ({ value: g, label: g }));
    return getInitialStringArray('subjects').map((g) => ({ value: g, label: g }));
  });
  const initIdentifiers = $derived.by(() => {
    const ids: IdentifierEntry[] = [];
    if (book?.identifiers) {
      Object.entries(book.identifiers).forEach(([key, value]) => ids.push({ key, value }));
    }
    // Merge identifiers from search results
    for (const result of searchResults) {
      if (result.identifiers) {
        for (const [key, value] of Object.entries(result.identifiers)) {
          if (typeof value === 'string' && !ids.some((id) => id.key === key)) {
            ids.push({ key, value });
          }
        }
      }
      if (result.external_id && result.provider) {
        const key = result.provider.toLowerCase().replace(/\s+/g, '_');
        if (!ids.some((id) => id.key === key)) {
          ids.push({ key, value: result.external_id });
        }
        // Hardcover's external_id is the slug — store it as hardcover_slug too
        // so IdentifierLinks can generate clickable URLs
        if (result.provider === 'hardcover' && !ids.some((id) => id.key === 'hardcover_slug')) {
          ids.push({ key: 'hardcover_slug', value: result.external_id });
        }
      }
    }
    return ids;
  });
  const initCoverUrl = $derived.by(() => {
    if (book?.cover_url) return book.cover_url;
    // Use priority-based selection for cover URL
    const sorted = sortedResults;
    for (const result of sorted) {
      if (result.cover_url) return result.cover_url;
    }
    return null;
  });

  // Helper to derive sort title from main title (move "The ", "A ", "An " to end)
  function deriveSortTitle(t: string): string {
    const prefixes = ['The ', 'A ', 'An '];
    for (const prefix of prefixes) {
      if (t.startsWith(prefix)) {
        return t.substring(prefix.length) + ', ' + prefix.trim();
      }
    }
    return t;
  }

  // Form state - each field initialized eagerly from derived initial values.
  // untrack() prevents these $derived reads from creating reactive subscriptions.
  let title = $state(untrack(() => initTitle) || '');
  let titleSource = $state<string | undefined>(
    untrack(() => (titleSources.length > 0 ? getInitialSourceForField('title') : undefined))
  );

  // Sort title is derived from title, but can be manually overridden
  let sortTitle = $state(
    untrack(() => {
      const t = initTitle;
      return t ? deriveSortTitle(t) : '';
    })
  );
  let _sortTitleManuallyEdited = $state(false);

  let authors = $state<AuthorInput[]>(untrack(() => (initAuthors.length > 0 ? initAuthors : [])));
  let selectedGenres = $state<{ value: string; label: string }[]>(
    untrack(() => (initGenres.length > 0 ? initGenres : []))
  );
  let seriesValue = $state(untrack(() => initSeries) || '');
  let seriesHardcoverId = $state<string | undefined>(undefined);
  let seriesIndex = $state<number | null>(untrack(() => initSeriesIndex));
  let seriesIndexSource = $state<string | undefined>(
    untrack(() =>
      seriesIndexSources.length > 0 ? getInitialSourceForField('series_index') : undefined
    )
  );
  let publisher = $state(untrack(() => initPublisher) || '');
  let publisherSource = $state<string | undefined>(
    untrack(() => (publisherSources.length > 0 ? getInitialSourceForField('publisher') : undefined))
  );
  let pubdate = $state(untrack(() => initPubdate) || '');
  let pubdateSource = $state<string | undefined>(
    untrack(() =>
      pubdateSources.length > 0 ? getInitialSourceForField('publish_date') : undefined
    )
  );
  let description = $state(untrack(() => initDescription) || '');
  let descriptionSource = $state<string | undefined>(
    untrack(() =>
      descriptionSources.length > 0 ? getInitialSourceForField('description') : undefined
    )
  );
  let language = $state(untrack(() => initLanguage) || '');
  let languageSource = $state<string | undefined>(
    untrack(() => (languageSources.length > 0 ? getInitialSourceForField('language') : undefined))
  );
  let pageCount = $state<number | null>(untrack(() => initPageCount));
  let pageCountSource = $state<string | undefined>(
    untrack(() =>
      pageCountSources.length > 0 ? getInitialSourceForField('page_count') : undefined
    )
  );
  let rating = $state<number | null>(untrack(() => initRating));
  let ratingSource = $state<string | undefined>(
    untrack(() => (ratingSources.length > 0 ? getInitialSourceForField('rating') : undefined))
  );
  let selectedCoverUrl = $state<string | null>(untrack(() => initCoverUrl));
  let coverFile = $state<File | null>(null);
  let identifiers = $state<IdentifierEntry[]>(
    untrack(() => (initIdentifiers.length > 0 ? initIdentifiers : []))
  );
  let newIdKey = $state('');
  let newIdValue = $state('');
  let showIdForm = $state(false);

  // Audiobook fields
  let narrator = $state(untrack(() => book?.narrator || ''));
  let isAbridged = $state(untrack(() => book?.is_abridged || false));
  let wantFormat = $state<'ebook' | 'audiobook' | 'both'>(
    untrack(() => book?.want_format || 'ebook')
  );

  // Convert authors to {value, label} format for MultiSearchSelect
  const authorsForSelect = $derived(authors.map((a) => ({ value: a.name, label: a.name })));

  // Handle author changes from MultiSearchSelect, preserving hardcover IDs
  function handleAuthorsChange(newValues: { value: string; label: string }[]) {
    authors = newValues.map((v) => {
      // Try to preserve existing hardcover ID for this author name
      const existing = authors.find((a) => a.name.toLowerCase() === v.label.toLowerCase());
      return {
        name: v.label,
        hardcover_id: existing?.hardcover_id
      };
    });
  }

  // Re-initialize form state when searchResults change after initial mount
  // (e.g., user opens edit mode, then searches metadata - results arrive after form was created)
  let prevSearchResultsRef: MetadataResult[] | null = null;
  $effect(() => {
    const current = searchResults;
    if (prevSearchResultsRef !== null && current !== prevSearchResultsRef && current.length > 0) {
      untrack(() => {
        title = initTitle;
        sortTitle = deriveSortTitle(initTitle);
        _sortTitleManuallyEdited = false;
        authors = initAuthors;
        seriesValue = initSeries;
        seriesHardcoverId = undefined;
        seriesIndex = initSeriesIndex;
        publisher = initPublisher;
        pubdate = initPubdate;
        description = initDescription;
        language = initLanguage;
        pageCount = initPageCount;
        rating = initRating;
        selectedGenres = initGenres;
        identifiers = initIdentifiers;
        selectedCoverUrl = initCoverUrl;
        coverFile = null;

        // Update source indicators
        titleSource = titleSources.length > 0 ? getInitialSourceForField('title') : undefined;
        publisherSource =
          publisherSources.length > 0 ? getInitialSourceForField('publisher') : undefined;
        pubdateSource =
          pubdateSources.length > 0 ? getInitialSourceForField('publish_date') : undefined;
        descriptionSource =
          descriptionSources.length > 0 ? getInitialSourceForField('description') : undefined;
        languageSource =
          languageSources.length > 0 ? getInitialSourceForField('language') : undefined;
        pageCountSource =
          pageCountSources.length > 0 ? getInitialSourceForField('page_count') : undefined;
        ratingSource = ratingSources.length > 0 ? getInitialSourceForField('rating') : undefined;
        seriesIndexSource =
          seriesIndexSources.length > 0 ? getInitialSourceForField('series_index') : undefined;

        // Reset complex field source indicators
        authorsCurrentSource = undefined;
        genresCurrentSource = undefined;
        seriesCurrentSource = undefined;
      });
    }
    prevSearchResultsRef = current;
  });

  // Dropdown state for authors/genres/series source selectors
  let authorsDropdownOpen = $state(false);
  let genresDropdownOpen = $state(false);
  let seriesDropdownOpen = $state(false);

  // Track current source for authors/genres/series (for icon display)
  // Start as undefined - only set when user selects from metadata dropdown
  let authorsCurrentSource = $state<string | undefined>(undefined);
  let genresCurrentSource = $state<string | undefined>(undefined);
  let seriesCurrentSource = $state<string | undefined>(undefined);

  // Deduplicate source values by value - only show first occurrence of each unique value
  function getUniqueSourceValues(sources: SourceValue[]): SourceValue[] {
    const seen = new Set<string>();
    return sources.filter((sv) => {
      const normalized = sv.value.trim().toLowerCase();
      if (seen.has(normalized)) return false;
      seen.add(normalized);
      return true;
    });
  }

  function getUniqueAuthorSourceValues(sources: AuthorSourceValue[]): AuthorSourceValue[] {
    const seen = new Set<string>();
    return sources.filter((sv) => {
      const normalized = sv.value.trim().toLowerCase();
      if (seen.has(normalized)) return false;
      seen.add(normalized);
      return true;
    });
  }

  function getUniqueSeriesSourceValues(sources: SeriesSourceValue[]): SeriesSourceValue[] {
    const seen = new Set<string>();
    return sources.filter((sv) => {
      const normalized = sv.value.trim().toLowerCase();
      if (seen.has(normalized)) return false;
      seen.add(normalized);
      return true;
    });
  }

  // Check if authors match a source value
  function authorsMatchSource(src: SourceValue): boolean {
    const currentAuthors = authors
      .map((a) => a.name.toLowerCase())
      .sort()
      .join(',');
    const srcAuthors = src.value
      .split(',')
      .map((a) => a.trim().toLowerCase())
      .sort()
      .join(',');
    return currentAuthors === srcAuthors;
  }

  // Check if genres match a source value
  function genresMatchSource(src: SourceValue): boolean {
    const currentGenres = selectedGenres
      .map((g) => g.label.toLowerCase())
      .sort()
      .join(',');
    const srcGenres = src.value
      .split(',')
      .map((g) => g.trim().toLowerCase())
      .sort()
      .join(',');
    return currentGenres === srcGenres;
  }

  // Color mapping for source tags
  const sourceColors: Record<string, string> = {
    GR: 'var(--accent)',
    GB: 'var(--muted)',
    HC: 'var(--monitored)',
    OL: 'var(--success)'
  };

  const getSourceColor = (source: string) => sourceColors[source] || 'var(--muted)';

  // Cover options from search results
  const coverOptions = $derived.by(() => {
    const options: { url: string; provider: string }[] = [];
    for (const result of searchResults) {
      if (result.cover_url) {
        options.push({
          url: result.cover_url,
          provider: getProviderName(result.provider)
        });
      }
    }
    return options;
  });

  function handleCoverSelect(url: string | null, file?: File) {
    selectedCoverUrl = url;
    coverFile = file || null;
  }

  const addIdentifier = () => {
    const key = newIdKey.trim().toLowerCase();
    const value = newIdValue.trim();
    if (key && value && !identifiers.some((id) => id.key === key)) {
      identifiers = [...identifiers, { key, value }];
      newIdKey = '';
      newIdValue = '';
      showIdForm = false;
    }
  };

  const removeIdentifier = (key: string) => {
    identifiers = identifiers.filter((id) => id.key !== key);
  };

  // Search functions for local data
  async function searchAuthorsLocal(
    query: string
  ): Promise<{ value: string; label: string; description?: string }[]> {
    if (!query || query.length < 2) return [];
    const results = await api.getAuthors({ search: query, limit: 10 });
    return results.items.map((a) => ({
      value: a.id,
      label: a.name,
      description: a.top_series_names || undefined
    }));
  }

  async function searchGenresLocal(query: string): Promise<{ value: string; label: string }[]> {
    if (!query || query.length < 2) return [];
    const allGenres = await api.getGenres();
    const q = query.toLowerCase();
    return allGenres
      .filter((g) => g.name.toLowerCase().includes(q))
      .map((g) => ({ value: g.name, label: g.name }));
  }

  async function searchSeriesLocal(
    query: string
  ): Promise<{ value: string; label: string; description?: string }[]> {
    if (!query || query.length < 2) return [];
    const results = await api.getSeries({ search: query, limit: 10 });
    return results.items.map((s) => ({
      value: s.id,
      label: s.name,
      description:
        s.authors && s.authors.length > 0
          ? s.authors
              .slice(0, 2)
              .map((a) => a.name)
              .join(', ') + (s.authors.length > 2 ? ` +${s.authors.length - 2}` : '')
          : undefined
    }));
  }

  // Track selected series ID separately from typed series name
  let selectedSeriesId = $state<string | undefined>(undefined);

  // Multi-series memberships (populated for existing books with junction data)
  let seriesMemberships = $state<SeriesMembership[]>(
    untrack(() => book?.series_memberships?.slice() || [])
  );
  const hasMultiSeries = $derived(seriesMemberships.length > 0);

  function addSeriesToMemberships(seriesId: string, seriesName: string, index: number | null) {
    if (seriesMemberships.some((m) => m.series_id === seriesId)) return;
    const isPrimary = seriesMemberships.length === 0;
    seriesMemberships = [
      ...seriesMemberships,
      { series_id: seriesId, series_name: seriesName, series_index: index, is_primary: isPrimary }
    ];
  }

  function removeSeriesMembership(seriesId: string) {
    const wasPrimary = seriesMemberships.find((m) => m.series_id === seriesId)?.is_primary;
    seriesMemberships = seriesMemberships.filter((m) => m.series_id !== seriesId);
    // If we removed the primary, promote the first remaining
    if (wasPrimary && seriesMemberships.length > 0) {
      seriesMemberships = seriesMemberships.map((m, i) => ({ ...m, is_primary: i === 0 }));
    }
  }

  function setMembershipPrimary(seriesId: string) {
    seriesMemberships = seriesMemberships.map((m) => ({
      ...m,
      is_primary: m.series_id === seriesId
    }));
  }

  function updateMembershipIndex(seriesId: string, index: number | null) {
    seriesMemberships = seriesMemberships.map((m) =>
      m.series_id === seriesId ? { ...m, series_index: index } : m
    );
  }

  // Handle source selection for authors - accepts the full AuthorSourceValue with IDs
  function handleAuthorsSourceSelect(sourceValue: AuthorSourceValue) {
    authors = sourceValue.authors;
    authorsCurrentSource = sourceValue.source;
  }

  function handleGenresSourceSelect(value: string, source?: string) {
    const genreNames = value
      .split(',')
      .map((g) => g.trim())
      .filter(Boolean);
    selectedGenres = genreNames.map((g) => ({ value: g, label: g }));
    if (source) genresCurrentSource = source;
  }

  function handleSeriesSourceSelect(sourceValue: SeriesSourceValue) {
    seriesValue = sourceValue.value;
    seriesHardcoverId = sourceValue.hardcover_id;
    seriesCurrentSource = sourceValue.source;
  }

  // Check if series matches a source value
  function seriesMatchesSource(src: SourceValue): boolean {
    return seriesValue.trim().toLowerCase() === src.value.trim().toLowerCase();
  }

  const displayImageUrl = $derived(selectedCoverUrl || '/placeholder-cover.png');

  const handleSave = async () => {
    if (!title.trim()) return;

    const idObj: Record<string, string> = {};
    identifiers.forEach(({ key, value }) => {
      if (key && value) idObj[key] = value;
    });

    // If extracted cover was selected, promote it to primary cover via dedicated endpoint
    const isExtractedCover = selectedCoverUrl && selectedCoverUrl.includes('/extracted-cover');
    if (isExtractedCover && book?.id) {
      await api.useExtractedCover(book.id);
    }

    // For remote cover URLs, let the backend download them (avoids CORS issues).
    // For existing books, use the dedicated cover-from-url endpoint.
    // For new books, pass the URL in the metadata so it's stored as source_url.
    let coverHandledByBackend = false;
    if (
      !isExtractedCover &&
      !coverFile &&
      selectedCoverUrl &&
      selectedCoverUrl !== book?.cover_url
    ) {
      if (book?.id) {
        await api.setCoverFromUrl(book.id, selectedCoverUrl);
        coverHandledByBackend = true;
      }
    }

    await onSave(
      {
        title: title.trim(),
        sort_title: sortTitle.trim() || undefined,
        authors: authors,
        series_id: hasMultiSeries ? undefined : selectedSeriesId || undefined,
        series_name: hasMultiSeries
          ? undefined
          : selectedSeriesId
            ? undefined
            : seriesValue.trim() || null,
        series_hardcover_id: hasMultiSeries ? undefined : seriesHardcoverId || null,
        series_index: hasMultiSeries ? undefined : seriesIndex,
        series_memberships: hasMultiSeries ? seriesMemberships : undefined,
        publisher: publisher.trim() || null,
        pubdate: pubdate.trim() || null,
        description: description.trim() || null,
        language: language.trim() || null,
        page_count: pageCount,
        identifiers: Object.keys(idObj).length > 0 ? idObj : undefined,
        genres: selectedGenres.length > 0 ? selectedGenres.map((g) => g.label) : undefined,
        rating: rating,
        cover_url:
          isExtractedCover || coverHandledByBackend || coverFile
            ? null
            : selectedCoverUrl !== book?.cover_url
              ? selectedCoverUrl
              : null,
        want: isCreateMode ? true : undefined,
        narrator: narrator.trim() || null,
        is_abridged: isAbridged,
        want_format: wantFormat
      },
      isExtractedCover || coverHandledByBackend ? undefined : coverFile || undefined
    );
  };

  // Expose save trigger for external footer buttons (sidebar mode)
  requestSave = handleSave;

  // Expose reactive save-ability for external footer buttons
  $effect(() => {
    canSave = !!title.trim() && !saving;
  });
</script>

<div class="book-form">
  {#if error}
    <div class="form-error">{error}</div>
  {/if}
  <div class="form-content">
    <div class="form-inner">
      <div class="form-layout">
        <!-- Cover selection row - inside scrolling area -->
        <div class="cover-row-inline">
          <label class="field-label" for="cover-selector">Cover</label>
          <CoverSelector
            id="cover-selector"
            currentUrl={book?.cover_url || null}
            extractedCoverUrl={book?.extracted_cover_url || null}
            searchCovers={coverOptions}
            selectedUrl={selectedCoverUrl}
            onSelect={handleCoverSelect}
          />
        </div>

        <!-- Left: Cover Preview -->
        <div class="image-section">
          <div class="image-preview">
            {#if selectedCoverUrl}
              <img src={displayImageUrl} alt="Cover" class="cover-img" />
            {:else}
              <span class="material-icons-outlined placeholder-icon">menu_book</span>
            {/if}
          </div>
        </div>

        <!-- Right: Fields -->
        <div class="fields-section">
          <!-- Title -->
          <div class="field-group">
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
              placeholder="Book title..."
            />
          </div>

          <!-- Sort Title -->
          <div class="field-group">
            <label class="field-label" for="sort-title-input">Sort Title</label>
            <input
              id="sort-title-input"
              type="text"
              class="form-input"
              value={sortTitle}
              oninput={(e) => {
                sortTitle = (e.target as HTMLInputElement).value;
                _sortTitleManuallyEdited = true;
              }}
              placeholder="Title, The"
            />
          </div>

          <!-- Authors -->

          <div class="field-group">
            <label class="field-label" for="authors-select">Authors</label>
            <div class="field-with-source">
              <div class="field-main">
                <MultiSearchSelect
                  id="authors-select"
                  placeholder="Search authors..."
                  searchFn={searchAuthorsLocal}
                  values={authorsForSelect}
                  onchange={(vals) =>
                    handleAuthorsChange(vals as { value: string; label: string }[])}
                  allowCreate={true}
                />
              </div>
              {#if authorsSources.length > 0}
                <div class="source-dropdown-container">
                  <button
                    type="button"
                    class="source-selector-btn"
                    onclick={() => (authorsDropdownOpen = !authorsDropdownOpen)}
                  >
                    {#if authorsCurrentSource}
                      <span
                        class="source-tag"
                        style="background: {getSourceColor(authorsCurrentSource)}"
                        >{authorsCurrentSource}</span
                      >
                    {/if}
                    <span class="material-icons-outlined">expand_more</span>
                  </button>
                  {#if authorsDropdownOpen}
                    <div class="source-dropdown">
                      {#each getUniqueAuthorSourceValues(authorsSources) as src, i (`${src.source}-${src.value}-${i}`)}
                        <button
                          type="button"
                          class="source-dropdown-item"
                          class:active={authorsMatchSource(src)}
                          onclick={() => {
                            handleAuthorsSourceSelect(src);
                            authorsDropdownOpen = false;
                          }}
                        >
                          <div class="source-dropdown-header">
                            <span
                              class="source-tag"
                              style="background: {getSourceColor(src.source)}">{src.source}</span
                            >
                            {src.sourceName}
                          </div>
                          <div class="source-dropdown-value">
                            {src.value.length > 60 ? src.value.slice(0, 60) + '...' : src.value}
                          </div>
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
          </div>

          <!-- Series & Index -->
          {#if hasMultiSeries}
            <!-- Multi-series list -->
            <div class="field-group" style="grid-column: 1 / -1;">
              <span class="field-label">Series</span>
              <div class="series-memberships-list">
                {#each seriesMemberships as membership (membership.series_id)}
                  <div class="series-membership-row">
                    <button
                      type="button"
                      class="series-primary-btn"
                      class:is-primary={membership.is_primary}
                      title={membership.is_primary ? 'Primary series' : 'Set as primary'}
                      onclick={() => setMembershipPrimary(membership.series_id)}
                    >
                      <span class="material-icons-outlined"
                        >{membership.is_primary ? 'star' : 'star_outline'}</span
                      >
                    </button>
                    <span class="series-membership-name">{membership.series_name}</span>
                    <input
                      type="number"
                      class="series-membership-index"
                      placeholder="#"
                      value={membership.series_index ?? ''}
                      onchange={(e) => {
                        const v = (e.target as HTMLInputElement).value;
                        updateMembershipIndex(membership.series_id, v ? parseFloat(v) : null);
                      }}
                    />
                    <button
                      type="button"
                      class="series-remove-btn"
                      title="Remove from series"
                      onclick={() => removeSeriesMembership(membership.series_id)}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                {/each}
                <div class="series-add-row">
                  <SearchSelect
                    id="series-add"
                    placeholder="Add series..."
                    searchFn={searchSeriesLocal}
                    bind:value={seriesValue}
                    allowCreate={true}
                    onchange={(val, item) => {
                      if (item && item.value !== item.label) {
                        addSeriesToMemberships(item.value, item.label, null);
                      } else if (val.trim()) {
                        // Created new series — use name as placeholder ID
                        addSeriesToMemberships(val.trim(), val.trim(), null);
                      }
                      seriesValue = '';
                    }}
                  />
                </div>
              </div>
            </div>
          {:else}
            <!-- Single series (create mode / no existing memberships) -->
            <div class="field-group series-name">
              <label class="field-label" for="series-select">Series</label>
              <div class="field-with-source">
                <div class="field-main">
                  <SearchSelect
                    id="series-select"
                    placeholder="Search series..."
                    searchFn={searchSeriesLocal}
                    bind:value={seriesValue}
                    allowCreate={true}
                    onchange={(val, item) => {
                      if (item && item.value !== item.label) {
                        selectedSeriesId = item.value;
                        seriesValue = item.label;
                      } else {
                        selectedSeriesId = undefined;
                      }
                    }}
                  />
                </div>
                {#if seriesSources.length > 0}
                  <div class="source-dropdown-container">
                    <button
                      type="button"
                      class="source-selector-btn"
                      onclick={() => (seriesDropdownOpen = !seriesDropdownOpen)}
                    >
                      {#if seriesCurrentSource}
                        <span
                          class="source-tag"
                          style="background: {getSourceColor(seriesCurrentSource)}"
                          >{seriesCurrentSource}</span
                        >
                      {/if}
                      <span class="material-icons-outlined">expand_more</span>
                    </button>
                    {#if seriesDropdownOpen}
                      <div class="source-dropdown">
                        {#each getUniqueSeriesSourceValues(seriesSources) as src, i (`${src.source}-${src.value}-${i}`)}
                          <button
                            type="button"
                            class="source-dropdown-item"
                            class:active={seriesMatchesSource(src)}
                            onclick={() => {
                              handleSeriesSourceSelect(src);
                              seriesDropdownOpen = false;
                            }}
                          >
                            <div class="source-dropdown-header">
                              <span
                                class="source-tag"
                                style="background: {getSourceColor(src.source)}">{src.source}</span
                              >
                              {src.sourceName}
                            </div>
                            <div class="source-dropdown-value">
                              {src.value.length > 60 ? src.value.slice(0, 60) + '...' : src.value}
                            </div>
                          </button>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
            <div class="field-group series-index">
              <SourceField
                label="#"
                value={seriesIndex?.toString() || ''}
                sourceValues={seriesIndexSources}
                currentSource={seriesIndexSource}
                type="number"
                onchange={(v, s) => {
                  seriesIndex = v ? parseFloat(v) : null;
                  if (s) seriesIndexSource = s;
                }}
                placeholder="1"
              />
            </div>
          {/if}

          <!-- Publisher & Year -->
          <div class="field-group publisher-name">
            <SourceField
              label="Publisher"
              value={publisher}
              sourceValues={publisherSources}
              currentSource={publisherSource}
              onchange={(v, s) => {
                publisher = v;
                if (s) publisherSource = s;
              }}
              placeholder="Publisher name..."
            />
          </div>
          <div class="field-group pub-year">
            <SourceField
              label="Year"
              value={pubdate}
              sourceValues={pubdateSources}
              currentSource={pubdateSource}
              onchange={(v, s) => {
                pubdate = v;
                if (s) pubdateSource = s;
              }}
              placeholder="2024"
            />
          </div>

          <!-- Language, Pages, Rating -->
          <div class="field-group language-code">
            <SourceField
              label="Language"
              value={language}
              sourceValues={languageSources}
              currentSource={languageSource}
              onchange={(v, s) => {
                language = v;
                if (s) languageSource = s;
              }}
              placeholder="en"
            />
          </div>
          <div class="field-group pages-count">
            <SourceField
              label="Pages"
              value={pageCount?.toString() || ''}
              sourceValues={pageCountSources}
              currentSource={pageCountSource}
              type="number"
              onchange={(v, s) => {
                pageCount = v ? parseInt(v) : null;
                if (s) pageCountSource = s;
              }}
              placeholder="—"
            />
          </div>
          <div class="field-group rating-value">
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
              placeholder="—"
            />
          </div>

          <!-- Audiobook Fields -->
          <div class="field-group narrator-name">
            <label class="field-label" for="narrator-input">Narrator</label>
            <input
              id="narrator-input"
              type="text"
              class="form-input"
              bind:value={narrator}
              placeholder="Narrator name..."
            />
          </div>
          <div class="field-group abridged-toggle">
            <label class="field-label" for="abridged-input">Abridged</label>
            <label class="toggle-label">
              <input
                id="abridged-input"
                type="checkbox"
                class="toggle-checkbox"
                bind:checked={isAbridged}
              />
              <span class="toggle-text">{isAbridged ? 'Yes' : 'No'}</span>
            </label>
          </div>
          <div class="field-group want-format-select">
            <label class="field-label" for="want-format-input">Want Format</label>
            <SearchSelect
              id="want-format-input"
              items={[
                { value: 'ebook', label: 'Ebook' },
                { value: 'audiobook', label: 'Audiobook' },
                { value: 'both', label: 'Both' }
              ]}
              value={wantFormat}
              onchange={(v) => {
                wantFormat = v as 'ebook' | 'audiobook' | 'both';
              }}
            />
          </div>

          <div class="field-group">
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
              placeholder="Book description..."
            />
          </div>

          <!-- Genres with source selector -->
          <div class="field-group">
            <label class="field-label" for="genres-select">Genres</label>
            <div class="field-with-source">
              <div class="field-main">
                <MultiSearchSelect
                  id="genres-select"
                  placeholder="Search genres..."
                  searchFn={searchGenresLocal}
                  values={selectedGenres}
                  onchange={(vals) => (selectedGenres = vals as { value: string; label: string }[])}
                  allowCreate={true}
                />
              </div>
              {#if genresSources.length > 0}
                <div class="source-dropdown-container">
                  <button
                    type="button"
                    class="source-selector-btn"
                    onclick={() => (genresDropdownOpen = !genresDropdownOpen)}
                  >
                    {#if genresCurrentSource}
                      <span
                        class="source-tag"
                        style="background: {getSourceColor(genresCurrentSource)}"
                        >{genresCurrentSource}</span
                      >
                    {/if}
                    <span class="material-icons-outlined">expand_more</span>
                  </button>
                  {#if genresDropdownOpen}
                    <div class="source-dropdown">
                      {#each getUniqueSourceValues(genresSources) as src, i (`${src.source}-${src.value}-${i}`)}
                        <button
                          type="button"
                          class="source-dropdown-item"
                          class:active={genresMatchSource(src)}
                          onclick={() => {
                            handleGenresSourceSelect(src.value, src.source);
                            genresDropdownOpen = false;
                          }}
                        >
                          <div class="source-dropdown-header">
                            <span
                              class="source-tag"
                              style="background: {getSourceColor(src.source)}">{src.source}</span
                            >
                            {src.sourceName}
                          </div>
                          <div class="source-dropdown-value">
                            {src.value.length > 60 ? src.value.slice(0, 60) + '...' : src.value}
                          </div>
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
          </div>

          <!-- Identifiers -->
          <div class="field-group">
            <div class="field-label-row">
              <label class="field-label" for="add-id-btn">Identifiers</label>
              <button
                id="add-id-btn"
                type="button"
                class="add-btn"
                onclick={() => (showIdForm = !showIdForm)}
              >
                <span class="material-icons-outlined">add</span>
              </button>
            </div>
            {#if showIdForm}
              <div class="id-add-form">
                <input
                  type="text"
                  class="form-input small"
                  bind:value={newIdKey}
                  placeholder="Key (e.g., isbn)"
                />
                <input
                  type="text"
                  class="form-input small"
                  bind:value={newIdValue}
                  placeholder="Value"
                />
                <button type="button" class="btn-sec small" onclick={addIdentifier}>Add</button>
              </div>
            {/if}
            {#if identifiers.length > 0}
              <div class="identifiers-list">
                {#each identifiers as id (id.key)}
                  <div class="identifier-chip">
                    <span class="id-key">{id.key}:</span>
                    <span class="id-value">{id.value}</span>
                    <button
                      type="button"
                      class="remove-btn"
                      onclick={() => removeIdentifier(id.key)}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                {/each}
              </div>
            {:else}
              <div class="empty-text">No identifiers</div>
            {/if}
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- Footer (hidden when parent provides external footer via ExpandableSidebar snippet) -->
  {#if !hideFooter}
    <div class="form-footer" class:fixed-footer={fixedFooter}>
      <div class={fixedFooter ? 'fixed-footer-inner' : 'footer-inline-content'}>
        {#if onDelete && !isCreateMode}
          <button
            type="button"
            class="btn btn-danger"
            onclick={onDelete}
            disabled={deleting || saving}
          >
            <span class="material-icons-outlined">delete</span>
            {deleting ? 'Deleting...' : isIngestMode ? 'Delete Item' : 'Delete Book'}
          </button>
        {/if}
        <div class="spacer"></div>
        <button type="button" class="btn btn-sec" onclick={onCancel} disabled={saving}>
          Cancel
        </button>
        <button
          type="button"
          class="btn btn-primary"
          onclick={handleSave}
          disabled={!title.trim() || saving}
        >
          <span class="material-icons-outlined">save</span>
          {saving ? 'Saving...' : isCreateMode ? 'Create Book' : 'Save Changes'}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .book-form {
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
    max-width: 1600px;
    margin: 0 auto;
    padding: 0 20px 40px;
  }

  .form-layout {
    display: grid;
    grid-template-columns: 200px 1fr;
    grid-auto-rows: min-content;
    gap: 32px;
    align-content: start;
  }

  .cover-row-inline {
    grid-column: 1 / -1;
    margin-bottom: -16px;
  }

  .image-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    grid-column: 1;
    position: sticky;
    top: 20px;
  }

  .image-preview {
    width: 200px;
    height: 300px;
    border-radius: 8px;
    background-color: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    overflow: hidden;
    box-shadow: var(--shadow);
  }

  .cover-img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .placeholder-icon {
    font-size: 40px;
    color: var(--muted);
    opacity: 0.5;
  }

  .fields-section {
    display: grid;
    grid-template-columns: repeat(12, 1fr);
    gap: 20px;
    min-width: 0;
    grid-column: 2;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    grid-column: span 12; /* Default to full width */
  }

  /* Specific field widths using the 12-column grid */
  .field-group.series-name {
    grid-column: span 10;
  }
  .field-group.series-index {
    grid-column: span 2;
  }

  .series-memberships-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .series-membership-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 6px;
  }

  .series-primary-btn {
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: var(--muted);
    display: flex;
    align-items: center;
  }

  .series-primary-btn .material-icons-outlined {
    font-size: 18px;
  }

  .series-primary-btn.is-primary {
    color: var(--accent);
  }

  .series-membership-name {
    flex: 1;
    font-size: 0.9em;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .series-membership-index {
    width: 56px;
    padding: 4px 6px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    text-align: center;
    font-size: 0.85em;
    background: transparent;
    color: var(--ink);
  }

  .series-remove-btn {
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: var(--muted);
    display: flex;
    align-items: center;
    opacity: 0.6;
  }

  .series-remove-btn:hover {
    opacity: 1;
    color: var(--danger);
  }

  .series-remove-btn .material-icons-outlined {
    font-size: 18px;
  }

  .series-add-row {
    margin-top: 4px;
  }

  .field-group.publisher-name {
    grid-column: span 10;
  }
  .field-group.pub-year {
    grid-column: span 2;
  }

  .field-group.language-code {
    grid-column: span 4;
  }
  .field-group.pages-count {
    grid-column: span 4;
  }
  .field-group.rating-value {
    grid-column: span 4;
  }

  .field-group.narrator-name {
    grid-column: span 6;
  }
  .field-group.abridged-toggle {
    grid-column: span 3;
  }
  .field-group.want-format-select {
    grid-column: span 3;
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    padding: 8px 0;
  }

  .toggle-checkbox {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .toggle-text {
    font-size: 13px;
    color: var(--ink);
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .field-label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  /* Field with integrated source selector (for MultiSearchSelect fields) */
  .field-with-source {
    display: flex;
    align-items: stretch;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    /* Don't use overflow: hidden - it clips the dropdown */
  }

  .field-with-source:focus-within {
    border-color: var(--accent);
  }

  .field-main {
    flex: 1;
    min-width: 0;
  }

  /* Remove the MultiSearchSelect's own border when inside field-with-source */
  .field-with-source :global(.multi-search-select-input-container) {
    border: none;
    border-radius: 8px 0 0 8px;
    background: transparent;
    min-height: 34px;
  }

  /* Remove the SearchSelect's own border when inside field-with-source */
  .field-with-source :global(.search-select) {
    border: none;
    border-radius: 0;
    background: transparent;
  }

  .field-with-source :global(.search-select input) {
    padding: 8px 10px;
    font-size: 13px;
    border: none;
    border-radius: 8px 0 0 8px;
  }

  /* Source dropdown container for authors/genres */
  .source-dropdown-container {
    position: relative;
    display: flex;
    align-items: center;
    border-left: 1px solid var(--ring);
    background: var(--bg-2);
    flex-shrink: 0;
    border-radius: 0 7px 7px 0;
  }

  .source-selector-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .source-selector-btn:hover {
    background: var(--card);
    color: var(--ink);
  }

  .source-selector-btn .material-icons-outlined {
    font-size: 16px;
  }

  .source-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    min-width: 280px;
    max-width: 400px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    z-index: 100;
    overflow: hidden;
  }

  .source-dropdown-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    padding: 10px 12px;
    border: none;
    border-bottom: 1px solid var(--ring);
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: inherit;
    font: inherit;
  }

  .source-dropdown-item:last-child {
    border-bottom: none;
  }

  .source-dropdown-item:hover {
    background: var(--bg-2);
  }

  .source-dropdown-header {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 500;
  }

  .source-dropdown-value {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.4;
    word-break: break-word;
  }

  .form-input {
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    width: 100%;
  }

  .form-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .form-input.small {
    padding: 6px 10px;
    font-size: 13px;
  }

  /* Override MultiSearchSelect to match form-input styling */
  .field-group :global(.multi-search-select-input-container) {
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--bg);
    min-height: 42px;
  }

  .field-group :global(.multi-search-select-input-container input) {
    font-size: 14px;
  }

  .field-group :global(.multi-search-select-input-container input::placeholder) {
    color: var(--muted);
  }

  .add-btn {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
  }

  .add-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .add-btn .material-icons-outlined {
    font-size: 16px;
  }

  .id-add-form {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }

  .identifiers-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
  }

  .identifier-chip {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 4px;
    font-size: 12px;
  }

  .id-key {
    color: var(--muted);
  }

  .id-value {
    color: var(--ink);
    font-family: monospace;
  }

  .remove-btn {
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
    margin-left: 4px;
  }

  .remove-btn:hover {
    color: var(--danger);
  }

  .remove-btn .material-icons-outlined {
    font-size: 14px;
  }

  .empty-text {
    font-size: 12px;
    color: var(--muted);
    font-style: italic;
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
    background: rgba(15, 17, 21, 0.85);
    backdrop-filter: blur(20px) saturate(1.8);
    -webkit-backdrop-filter: blur(20px) saturate(1.8);
    border-top: 1px solid var(--ring);
    z-index: 1001;
    display: flex;
    align-items: center;
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.3);
  }

  .fixed-footer-inner {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  :global([data-theme='light']) .form-footer.fixed-footer {
    background: rgba(245, 239, 231, 0.85);
    box-shadow: 0 -10px 40px rgba(0, 0, 0, 0.05);
  }

  /* Responsive styles for mobile */
  @media (max-width: 768px) {
    .form-layout {
      grid-template-columns: 1fr;
      gap: 16px;
    }

    .image-section {
      flex-direction: row;
      align-items: flex-start;
      gap: 16px;
      position: static;
      grid-column: 1;
    }

    .image-preview {
      width: 100px;
      height: 150px;
      flex-shrink: 0;
    }

    .fields-section {
      grid-column: 1;
      grid-template-columns: repeat(12, 1fr);
    }

    .field-group {
      grid-column: span 12;
    }

    .field-group.series-name {
      grid-column: span 9;
    }

    .field-group.publisher-name {
      grid-column: span 12;
    }

    .field-group.series-index {
      grid-column: span 3;
    }

    .field-group.pub-year,
    .field-group.language-code,
    .field-group.pages-count,
    .field-group.rating-value {
      grid-column: span 6;
    }
  }
</style>
