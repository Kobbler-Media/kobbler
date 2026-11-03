<script lang="ts">
  /**
   * SourceField - A form field with a source selector dropdown.
   * Shows values from different metadata sources and allows switching between them.
   */
  import { getProviderColor } from '#lib/utils/format.js';

  type SourceValue = {
    source: string; // e.g., 'GR', 'GB', 'HC'
    sourceName: string; // e.g., 'Goodreads', 'Google Books'
    value: string;
  };

  interface Props {
    label: string;
    value: string;
    sourceValues?: SourceValue[];
    currentSource?: string;
    onchange: (value: string, source?: string) => void;
    type?: 'text' | 'textarea' | 'number';
    placeholder?: string;
    disabled?: boolean;
  }

  let {
    label,
    value,
    sourceValues = [],
    currentSource,
    onchange,
    type = 'text',
    placeholder = '',
    disabled = false
  }: Props = $props();

  let dropdownOpen = $state(false);
  let dropdownRef = $state<HTMLDivElement | null>(null);

  // Click outside handler
  const handleClickOutside = (event: MouseEvent) => {
    if (dropdownRef && !dropdownRef.contains(event.target as Node)) {
      dropdownOpen = false;
    }
  };

  $effect(() => {
    if (dropdownOpen) {
      // Use setTimeout to avoid closing immediately on the click that opened it
      setTimeout(() => {
        document.addEventListener('click', handleClickOutside);
      }, 0);
      return () => {
        document.removeEventListener('click', handleClickOutside);
      };
    }
  });

  // Show dropdown if:
  // 1. There are multiple distinct source values, OR
  // 2. Current value differs from any source value (user entered something different), OR
  // 3. Current value is empty and there are source values to select from
  const shouldShowDropdown = $derived(() => {
    if (sourceValues.length === 0) return false;
    const uniqueValues = new Set(sourceValues.map((sv) => sv.value.trim().toLowerCase()));
    // Multiple distinct values from sources
    if (uniqueValues.size > 1) return true;
    const currentNormalized = value.trim().toLowerCase();
    // Current value is empty but sources have values to offer
    if (!currentNormalized) return true;
    // Current value differs from the source value(s)
    if (!uniqueValues.has(currentNormalized)) return true;
    return false;
  });

  // Deduplicate sourceValues by value - only show first occurrence of each unique value
  const uniqueSourceValues = $derived(() => {
    const seen = new Set<string>();
    return sourceValues.filter((sv) => {
      const normalized = sv.value.trim().toLowerCase();
      if (seen.has(normalized)) return false;
      seen.add(normalized);
      return true;
    });
  });

  const selectSource = (sv: SourceValue) => {
    onchange(sv.value, sv.source);
    dropdownOpen = false;
  };

  // Check if this source value is currently selected (match by value, not just source)
  const isSelected = (sv: SourceValue) => {
    return value.trim().toLowerCase() === sv.value.trim().toLowerCase();
  };

  const handleInputChange = (e: Event) => {
    const target = e.target as HTMLInputElement | HTMLTextAreaElement;
    onchange(target.value);
  };

  const getSourceColor = (source: string) => getProviderColor(source);
</script>

<div class="source-field-container">
  <div class="source-field-label">{label}</div>
  <div class="source-field">
    {#if type === 'textarea'}
      <textarea
        class="source-field-input"
        {value}
        {placeholder}
        {disabled}
        oninput={handleInputChange}></textarea>
    {:else if type === 'number'}
      <input
        class="source-field-input"
        type="number"
        {value}
        {placeholder}
        {disabled}
        oninput={handleInputChange}
      />
    {:else}
      <input
        class="source-field-input"
        type="text"
        {value}
        {placeholder}
        {disabled}
        oninput={handleInputChange}
      />
    {/if}

    {#if shouldShowDropdown()}
      <div class="source-selector-wrapper" bind:this={dropdownRef}>
        <button
          type="button"
          class="source-selector"
          onclick={() => (dropdownOpen = !dropdownOpen)}
          {disabled}
        >
          {#if currentSource}
            <span class="source-tag" style="background: {getSourceColor(currentSource)}"
              >{currentSource}</span
            >
          {/if}
          <span class="material-icons-outlined">expand_more</span>
        </button>

        {#if dropdownOpen}
          <div class="source-dropdown">
            {#each uniqueSourceValues() as sv, i (`${sv.source}-${sv.value}-${i}`)}
              <button
                type="button"
                class="source-dropdown-item"
                class:active={isSelected(sv)}
                onclick={() => selectSource(sv)}
              >
                <div class="source-dropdown-header">
                  <span class="source-tag" style="background: {getSourceColor(sv.source)}"
                    >{sv.source}</span
                  >
                  {sv.sourceName}
                </div>
                <div class="source-dropdown-value">
                  {sv.value.length > 100 ? sv.value.slice(0, 100) + '...' : sv.value}
                </div>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .source-field-container {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .source-field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .source-field {
    display: flex;
    align-items: stretch;
    position: relative;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    overflow: visible;
  }

  .source-field:focus-within {
    border-color: var(--accent);
  }

  .source-field-input {
    flex: 1;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
    min-width: 0;
    border-radius: 8px 0 0 8px;
  }

  .source-field-input:focus {
    outline: none;
  }

  textarea.source-field-input {
    resize: vertical;
    min-height: 80px;
  }

  /* Hide number input spinners */
  input[type='number'].source-field-input {
    -moz-appearance: textfield;
    appearance: textfield;
  }

  input[type='number'].source-field-input::-webkit-outer-spin-button,
  input[type='number'].source-field-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }

  .source-selector-wrapper {
    position: relative;
    display: flex;
    flex-shrink: 0;
  }

  .source-selector {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 8px;
    border: none;
    border-left: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
    border-radius: 0 7px 7px 0;
  }

  .source-selector:hover {
    background: var(--card);
    color: var(--ink);
  }

  .source-selector .material-icons-outlined {
    font-size: 16px;
  }

  .source-tag {
    display: inline-block;
    font-size: 9px;
    font-weight: 700;
    padding: 2px 5px;
    border-radius: 3px;
    color: #000;
    text-transform: uppercase;
    letter-spacing: 0.3px;
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

  .source-dropdown-item.active {
    background: rgba(211, 140, 90, 0.1);
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
</style>
