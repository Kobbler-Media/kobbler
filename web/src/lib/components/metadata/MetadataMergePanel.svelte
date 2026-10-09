<script lang="ts">
  import type { MetadataResult } from '$lib/api/client';
  import { type OverrideableField, getFieldOptions } from './types';
  import { getProviderName } from '$lib/utils/format';

  interface Props {
    results: MetadataResult[];
    selectedForMerge: number[];
    mergePreview: MetadataResult | null;
    fieldOverrides: Partial<Record<OverrideableField, number>>;
    onSetFieldOverride: (field: OverrideableField, index: number) => void;
  }

  let { results, selectedForMerge, mergePreview, fieldOverrides, onSetFieldOverride }: Props =
    $props();

  // Track which dropdown is open
  let openDropdown = $state<string | null>(null);

  const toggleDropdown = (field: string) => {
    openDropdown = openDropdown === field ? null : field;
  };

  const getOptions = (field: OverrideableField) =>
    getFieldOptions(field, selectedForMerge, results);

  const getSelectedIndex = (field: OverrideableField): number | null => {
    if (fieldOverrides[field] !== undefined) {
      return fieldOverrides[field]!;
    }
    const options = getOptions(field);
    return options.length > 0 ? options[0].index : null;
  };

  const handleSetOverride = (field: OverrideableField, index: number) => {
    openDropdown = null;
    onSetFieldOverride(field, index);
  };
</script>

<div class="merge-preview">
  <div class="merge-preview-header">
    <h4>Merge Preview</h4>
    <span class="merge-hint">Click fields to change source</span>
  </div>
  {#if mergePreview}
    <div class="merge-preview-content">
      <!-- Cover Selection -->
      {#if getOptions('cover_url').length > 0}
        {@const coverOptions = getOptions('cover_url')}
        <div class="merge-field-row">
          <span class="merge-label">Cover</span>
          <div class="cover-selector">
            {#each coverOptions as opt (opt.index)}
              <button
                type="button"
                class="cover-option"
                class:selected={getSelectedIndex('cover_url') === opt.index}
                onclick={() => handleSetOverride('cover_url', opt.index)}
              >
                <img src={opt.value as string} alt="" />
                <span class="cover-provider">{getProviderName(opt.provider)}</span>
              </button>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Title -->
      {#if getOptions('title').length > 1}
        {@const titleOptions = getOptions('title')}
        <div class="merge-field-row">
          <span class="merge-label">Title</span>
          <div class="field-dropdown-wrapper">
            <button
              type="button"
              class="field-dropdown-trigger"
              onclick={() => toggleDropdown('title')}
            >
              <span class="field-value">{mergePreview.title}</span>
              <span class="field-source"
                >{getProviderName(results[getSelectedIndex('title')!].provider)}</span
              >
              <svg class="dropdown-arrow" viewBox="0 0 24 24" width="12" height="12"
                ><path d="M7 10l5 5 5-5H7z" fill="currentColor"></path></svg
              >
            </button>
            {#if openDropdown === 'title'}
              <div class="field-dropdown-menu">
                {#each titleOptions as opt (opt.index)}
                  <button
                    type="button"
                    class="dropdown-option"
                    class:selected={getSelectedIndex('title') === opt.index}
                    onclick={() => handleSetOverride('title', opt.index)}
                  >
                    <span class="option-value">{opt.value}</span>
                    <span class="option-provider">{getProviderName(opt.provider)}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if mergePreview.title}
        <div class="merge-field-row">
          <span class="merge-label">Title</span>
          <span class="merge-value-static">{mergePreview.title}</span>
        </div>
      {/if}

      <!-- Authors -->
      {#if getOptions('authors').length > 1}
        {@const authorOptions = getOptions('authors')}
        <div class="merge-field-row">
          <span class="merge-label">Authors</span>
          <div class="field-dropdown-wrapper">
            <button
              type="button"
              class="field-dropdown-trigger"
              onclick={() => toggleDropdown('authors')}
            >
              <span class="field-value">{mergePreview.authors.join(', ')}</span>
              <span class="field-source"
                >{getProviderName(results[getSelectedIndex('authors')!].provider)}</span
              >
              <svg class="dropdown-arrow" viewBox="0 0 24 24" width="12" height="12"
                ><path d="M7 10l5 5 5-5H7z" fill="currentColor"></path></svg
              >
            </button>
            {#if openDropdown === 'authors'}
              <div class="field-dropdown-menu">
                {#each authorOptions as opt (opt.index)}
                  <button
                    type="button"
                    class="dropdown-option"
                    class:selected={getSelectedIndex('authors') === opt.index}
                    onclick={() => handleSetOverride('authors', opt.index)}
                  >
                    <span class="option-value">{(opt.value as string[]).join(', ')}</span>
                    <span class="option-provider">{getProviderName(opt.provider)}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if mergePreview.authors.length}
        <div class="merge-field-row">
          <span class="merge-label">Authors</span>
          <span class="merge-value-static">{mergePreview.authors.join(', ')}</span>
        </div>
      {/if}

      <!-- Description -->
      {#if getOptions('description').length > 0}
        {@const descOptions = getOptions('description')}
        <div class="merge-field-row merge-field-description">
          <span class="merge-label">Description</span>
          <div class="description-selector">
            <button
              type="button"
              class="description-trigger"
              onclick={() => toggleDropdown('description')}
            >
              <span class="description-preview">{mergePreview.description?.slice(0, 80)}...</span>
              <span class="field-source"
                >{getProviderName(results[getSelectedIndex('description')!].provider)}</span
              >
              <svg
                class="dropdown-arrow"
                class:rotated={openDropdown === 'description'}
                viewBox="0 0 24 24"
                width="12"
                height="12"><path d="M7 10l5 5 5-5H7z" fill="currentColor"></path></svg
              >
            </button>
            {#if openDropdown === 'description'}
              <div class="description-dropdown">
                {#each descOptions as opt (opt.index)}
                  <button
                    type="button"
                    class="description-option"
                    class:selected={getSelectedIndex('description') === opt.index}
                    onclick={() => handleSetOverride('description', opt.index)}
                  >
                    <div class="description-option-header">
                      <span class="option-provider">{getProviderName(opt.provider)}</span>
                      {#if getSelectedIndex('description') === opt.index}
                        <span class="selected-badge">Selected</span>
                      {/if}
                    </div>
                    <p class="description-full">{opt.value}</p>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Series -->
      {#if getOptions('series').length > 1}
        {@const seriesOptions = getOptions('series')}
        <div class="merge-field-row">
          <span class="merge-label">Series</span>
          <div class="field-dropdown-wrapper">
            <button
              type="button"
              class="field-dropdown-trigger"
              onclick={() => toggleDropdown('series')}
            >
              <span class="field-value"
                >{mergePreview.series}{mergePreview.series_index
                  ? ` #${mergePreview.series_index}`
                  : ''}</span
              >
              <span class="field-source"
                >{getProviderName(results[getSelectedIndex('series')!].provider)}</span
              >
              <svg class="dropdown-arrow" viewBox="0 0 24 24" width="12" height="12"
                ><path d="M7 10l5 5 5-5H7z" fill="currentColor"></path></svg
              >
            </button>
            {#if openDropdown === 'series'}
              <div class="field-dropdown-menu">
                {#each seriesOptions as opt (opt.index)}
                  {@const r = results[opt.index]}
                  <button
                    type="button"
                    class="dropdown-option"
                    class:selected={getSelectedIndex('series') === opt.index}
                    onclick={() => handleSetOverride('series', opt.index)}
                  >
                    <span class="option-value"
                      >{opt.value}{r.series_index ? ` #${r.series_index}` : ''}</span
                    >
                    <span class="option-provider">{getProviderName(opt.provider)}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if mergePreview.series}
        <div class="merge-field-row">
          <span class="merge-label">Series</span>
          <span class="merge-value-static"
            >{mergePreview.series}{mergePreview.series_index
              ? ` #${mergePreview.series_index}`
              : ''}</span
          >
        </div>
      {/if}

      <!-- Rating -->
      {#if getOptions('rating').length > 1}
        {@const ratingOptions = getOptions('rating')}
        <div class="merge-field-row">
          <span class="merge-label">Rating</span>
          <div class="field-dropdown-wrapper">
            <button
              type="button"
              class="field-dropdown-trigger"
              onclick={() => toggleDropdown('rating')}
            >
              <span class="field-value"
                >{mergePreview.rating?.toFixed(1)}/5{mergePreview.rating_count
                  ? ` (${mergePreview.rating_count.toLocaleString()})`
                  : ''}</span
              >
              <span class="field-source"
                >{getProviderName(results[getSelectedIndex('rating')!].provider)}</span
              >
              <svg class="dropdown-arrow" viewBox="0 0 24 24" width="12" height="12"
                ><path d="M7 10l5 5 5-5H7z" fill="currentColor"></path></svg
              >
            </button>
            {#if openDropdown === 'rating'}
              <div class="field-dropdown-menu">
                {#each ratingOptions as opt (opt.index)}
                  {@const r = results[opt.index]}
                  <button
                    type="button"
                    class="dropdown-option"
                    class:selected={getSelectedIndex('rating') === opt.index}
                    onclick={() => handleSetOverride('rating', opt.index)}
                  >
                    <span class="option-value"
                      >{(opt.value as number).toFixed(1)}/5{r.rating_count
                        ? ` (${r.rating_count.toLocaleString()})`
                        : ''}</span
                    >
                    <span class="option-provider">{getProviderName(opt.provider)}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if mergePreview.rating}
        <div class="merge-field-row">
          <span class="merge-label">Rating</span>
          <span class="merge-value-static"
            >{mergePreview.rating.toFixed(1)}/5{mergePreview.rating_count
              ? ` (${mergePreview.rating_count.toLocaleString()})`
              : ''}</span
          >
        </div>
      {/if}

      <!-- Subjects -->
      {#if mergePreview.subjects && mergePreview.subjects.length > 0}
        <div class="merge-field-row">
          <span class="merge-label">Subjects</span>
          <div class="subjects-combined">
            {#each mergePreview.subjects.slice(0, 6) as subject, i (`${i}-${subject}`)}
              <span class="subject-chip-small">{subject}</span>
            {/each}
            {#if mergePreview.subjects.length > 6}
              <span class="subject-more-small">+{mergePreview.subjects.length - 6}</span>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .merge-preview {
    background: rgba(211, 140, 90, 0.08);
    border: 1px solid var(--accent);
    border-radius: 10px;
    padding: 12px;
  }

  .merge-preview-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .merge-preview-header h4 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
  }

  .merge-hint {
    font-size: 10px;
    color: var(--muted);
  }

  .merge-preview-content {
    display: grid;
    gap: 8px;
  }

  .merge-field-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 6px 0;
    border-bottom: 1px solid rgba(211, 140, 90, 0.15);
  }

  .merge-field-row:last-child {
    border-bottom: none;
  }

  .merge-field-row .merge-label {
    width: 70px;
    flex-shrink: 0;
    font-size: 11px;
    color: var(--muted);
    padding-top: 4px;
  }

  .merge-value-static {
    flex: 1;
    font-size: 12px;
    color: var(--ink);
  }

  /* Cover selector */
  .cover-selector {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding: 4px 0;
    scrollbar-width: thin;
  }

  .cover-option {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 4px;
    border: 2px solid transparent;
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .cover-option:hover {
    background: rgba(211, 140, 90, 0.08);
  }

  .cover-option.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.12);
  }

  .cover-option img {
    width: 45px;
    height: 65px;
    object-fit: cover;
    border-radius: 4px;
  }

  .cover-provider {
    font-size: 9px;
    color: var(--muted);
    text-transform: capitalize;
  }

  .cover-option.selected .cover-provider {
    color: var(--accent);
    font-weight: 600;
  }

  /* Field dropdown */
  .field-dropdown-wrapper {
    flex: 1;
    position: relative;
  }

  .field-dropdown-trigger {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .field-dropdown-trigger:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.04);
  }

  .field-value {
    flex: 1;
    font-size: 12px;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .field-source {
    flex-shrink: 0;
    padding: 1px 6px;
    background: var(--ring);
    border-radius: 4px;
    font-size: 9px;
    color: var(--muted);
    text-transform: capitalize;
  }

  .dropdown-arrow {
    flex-shrink: 0;
    color: var(--muted);
    transition: transform 0.2s;
  }

  .dropdown-arrow.rotated {
    transform: rotate(180deg);
  }

  .field-dropdown-menu {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 100;
    margin-top: 4px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
  }

  .dropdown-option {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    transition: background 0.15s;
  }

  .dropdown-option:hover {
    background: rgba(211, 140, 90, 0.08);
  }

  .dropdown-option.selected {
    background: rgba(211, 140, 90, 0.12);
  }

  .dropdown-option + .dropdown-option {
    border-top: 1px solid var(--ring);
  }

  .option-value {
    flex: 1;
    font-size: 12px;
    color: var(--ink);
  }

  .option-provider {
    flex-shrink: 0;
    padding: 1px 6px;
    background: var(--ring);
    border-radius: 4px;
    font-size: 9px;
    color: var(--muted);
    text-transform: capitalize;
  }

  .dropdown-option.selected .option-provider {
    background: var(--accent);
    color: white;
  }

  /* Description selector */
  .merge-field-description {
    flex-direction: column;
    align-items: stretch;
  }

  .merge-field-description .merge-label {
    width: auto;
    padding-top: 0;
    margin-bottom: 4px;
  }

  .description-selector {
    position: relative;
  }

  .description-trigger {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .description-trigger:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.04);
  }

  .description-preview {
    flex: 1;
    font-size: 11px;
    color: var(--ink);
    line-height: 1.4;
  }

  .description-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 100;
    margin-top: 4px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    max-height: 300px;
    overflow-y: auto;
  }

  .description-option {
    width: 100%;
    display: block;
    padding: 10px 12px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    transition: background 0.15s;
  }

  .description-option:hover {
    background: rgba(211, 140, 90, 0.08);
  }

  .description-option.selected {
    background: rgba(211, 140, 90, 0.12);
  }

  .description-option + .description-option {
    border-top: 1px solid var(--ring);
  }

  .description-option-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }

  .selected-badge {
    padding: 1px 6px;
    background: var(--accent);
    color: white;
    border-radius: 4px;
    font-size: 9px;
    font-weight: 600;
  }

  .description-full {
    margin: 0;
    font-size: 11px;
    line-height: 1.5;
    color: var(--ink);
    max-height: 100px;
    overflow-y: auto;
  }

  /* Subjects */
  .subjects-combined {
    flex: 1;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .subject-chip-small {
    padding: 2px 6px;
    background: var(--ring);
    border-radius: 10px;
    font-size: 10px;
    color: var(--ink);
  }

  .subject-more-small {
    padding: 2px 6px;
    font-size: 10px;
    color: var(--muted);
  }
</style>
