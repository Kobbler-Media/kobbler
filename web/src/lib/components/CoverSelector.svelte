<script lang="ts">
  // Cover option type for potential future use
  interface _CoverOption {
    url: string;
    label: string;
    source: 'current' | 'extracted' | 'search' | 'url' | 'upload';
  }

  interface Props {
    id?: string;
    label?: string;
    currentUrl?: string | null;
    extractedCoverUrl?: string | null;
    searchCovers?: { url: string; provider: string }[];
    selectedUrl?: string | null;
    onSelect: (url: string | null, file?: File) => void;
    aspectRatio?: 'portrait' | 'landscape';
  }

  let {
    id,
    label,
    currentUrl,
    extractedCoverUrl,
    searchCovers = [],
    selectedUrl,
    onSelect,
    aspectRatio = 'portrait'
  }: Props = $props();

  const isLandscape = $derived(aspectRatio === 'landscape');

  let urlInput = $state('');
  let urlPreviewValid = $state(false);
  let urlFocused = $state(false);
  let fileInput: HTMLInputElement | null = $state(null);
  let urlInputEl: HTMLInputElement | null = $state(null);
  let uploadPreviewUrl = $state<string | null>(null);

  // Determine which option is currently selected
  const getSelectedSource = (): string | null => {
    if (!selectedUrl) return null;
    if (currentUrl && selectedUrl === currentUrl) return 'current';
    if (extractedCoverUrl && selectedUrl === extractedCoverUrl) return 'extracted';
    if (searchCovers.some((c) => c.url === selectedUrl)) return 'search';
    if (uploadPreviewUrl && selectedUrl === uploadPreviewUrl) return 'upload';
    return 'url';
  };

  const handleCurrentSelect = () => {
    if (currentUrl) {
      onSelect(currentUrl);
    }
  };

  const handleExtractedSelect = () => {
    if (extractedCoverUrl) {
      onSelect(extractedCoverUrl);
    }
  };

  const handleSearchSelect = (url: string) => {
    onSelect(url);
  };

  const handleUrlChange = () => {
    if (urlInput && isValidUrl(urlInput)) {
      urlPreviewValid = true;
      onSelect(urlInput);
    } else {
      urlPreviewValid = false;
    }
  };

  const isValidUrl = (str: string): boolean => {
    try {
      new URL(str);
      return str.startsWith('http://') || str.startsWith('https://');
    } catch {
      return false;
    }
  };

  const handleFileSelect = (event: Event) => {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;

    // Clean up previous preview
    if (uploadPreviewUrl) {
      URL.revokeObjectURL(uploadPreviewUrl);
    }

    uploadPreviewUrl = URL.createObjectURL(file);
    onSelect(uploadPreviewUrl, file);
  };

  const triggerUpload = () => {
    fileInput?.click();
  };

  // Cleanup on unmount
  $effect(() => {
    return () => {
      if (uploadPreviewUrl) {
        URL.revokeObjectURL(uploadPreviewUrl);
      }
    };
  });
</script>

<div
  class="cover-selector"
  class:landscape={isLandscape}
  style="--img-w: {isLandscape ? 96 : 50}px; --img-h: {isLandscape
    ? 54
    : 72}px; --opt-w: {isLandscape ? 116 : 72}px; --url-w: {isLandscape
    ? 130
    : 90}px; --url-fw: {isLandscape ? 180 : 140}px;"
>
  {#if label}
    <span class="selector-label">{label}</span>
  {/if}
  <div class="cover-options">
    <!-- Current cover -->
    {#if currentUrl}
      <button
        type="button"
        class="cover-option"
        class:selected={getSelectedSource() === 'current'}
        onclick={handleCurrentSelect}
      >
        <img src={currentUrl} alt="Current" />
        <span class="cover-label">Current</span>
      </button>
    {/if}

    <!-- Extracted cover from ebook file -->
    {#if extractedCoverUrl}
      <button
        type="button"
        class="cover-option"
        class:selected={getSelectedSource() === 'extracted'}
        onclick={handleExtractedSelect}
      >
        <img src={extractedCoverUrl} alt="Extracted cover" />
        <span class="cover-label">Extracted</span>
      </button>
    {/if}

    <!-- Search result covers -->
    {#each searchCovers as cover, i (cover.url + ':' + i)}
      <button
        type="button"
        class="cover-option"
        class:selected={selectedUrl === cover.url}
        onclick={() => handleSearchSelect(cover.url)}
      >
        <img src={cover.url} alt={cover.provider} />
        <span class="cover-label">{cover.provider}</span>
      </button>
    {/each}

    <!-- URL input option -->
    <button
      type="button"
      class="cover-option url-option"
      class:url-focused={urlFocused}
      class:selected={getSelectedSource() === 'url'}
      onclick={() => urlInputEl?.focus()}
    >
      <div class="url-preview">
        {#if urlInput && urlPreviewValid}
          <img src={urlInput} alt="URL preview" />
        {:else}
          <div class="url-placeholder">
            <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
              <path
                d="M3.9 12c0-1.71 1.39-3.1 3.1-3.1h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1zM8 13h8v-2H8v2zm9-6h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1s-1.39 3.1-3.1 3.1h-4V17h4c2.76 0 5-2.24 5-5s-2.24-5-5-5z"
              ></path>
            </svg>
          </div>
        {/if}
      </div>
      <input
        {id}
        type="text"
        placeholder="Paste image URL"
        class="url-input"
        bind:value={urlInput}
        bind:this={urlInputEl}
        oninput={handleUrlChange}
        onfocus={() => (urlFocused = true)}
        onblur={() => (urlFocused = false)}
        onclick={(e) => e.stopPropagation()}
      />
    </button>

    <!-- Upload option -->
    <button type="button" class="cover-option upload-option" onclick={triggerUpload}>
      {#if uploadPreviewUrl}
        <img src={uploadPreviewUrl} alt="Upload preview" />
      {:else}
        <div class="upload-placeholder">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
            <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"></path>
          </svg>
        </div>
      {/if}
      <span class="cover-label">Upload</span>
    </button>
  </div>

  <!-- Hidden file input -->
  <input
    type="file"
    accept="image/*"
    class="hidden-input"
    bind:this={fileInput}
    onchange={handleFileSelect}
  />
</div>

<style>
  .cover-selector {
    width: 100%;
  }

  .selector-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .cover-options {
    display: flex;
    gap: 10px;
    overflow-x: auto;
    padding: 4px 0;
    scrollbar-width: thin;
  }

  .cover-option {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 6px;
    border: 2px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background 0.15s;
    width: var(--opt-w, 72px);
  }

  .cover-option:hover {
    background: rgba(211, 140, 90, 0.08);
    border-color: var(--accent);
  }

  .cover-option.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.12);
  }

  .cover-option img {
    width: var(--img-w, 50px);
    height: var(--img-h, 72px);
    object-fit: cover;
    border-radius: 4px;
    background: var(--bg);
  }

  .cover-label {
    font-size: 9px;
    color: var(--muted);
    text-transform: capitalize;
    text-align: center;
    line-height: 1.2;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cover-option.selected .cover-label {
    color: var(--accent);
    font-weight: 600;
  }

  /* URL input option */
  .url-option {
    width: var(--url-w, 90px);
    transition:
      width 0.2s ease,
      border-color 0.15s,
      background 0.15s;
  }

  .url-option.url-focused {
    width: var(--url-fw, 140px);
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  .url-preview {
    width: var(--img-w, 50px);
    height: var(--img-h, 72px);
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    background: var(--bg);
    border: 1px dashed var(--ring);
    transition: border-color 0.15s;
  }

  .url-option.url-focused .url-preview {
    border-color: var(--accent);
  }

  .url-preview img {
    width: 100%;
    height: 100%;
    border: none;
  }

  .url-placeholder {
    color: var(--muted);
    transition: color 0.15s;
  }

  .url-option.url-focused .url-placeholder {
    color: var(--accent);
  }

  .url-input {
    width: 100%;
    padding: 4px 6px;
    font-size: 11px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    background: transparent;
    color: var(--ink);
    text-align: center;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .url-input:focus {
    outline: none;
    border-color: var(--accent);
    background: rgba(255, 255, 255, 0.05);
  }

  .url-input::placeholder {
    color: var(--muted);
  }

  /* Upload option */
  .upload-option {
    cursor: pointer;
  }

  .upload-placeholder {
    width: var(--img-w, 50px);
    height: var(--img-h, 72px);
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    background: var(--bg);
    border: 1px dashed var(--ring);
    color: var(--muted);
    transition: all 0.15s;
  }

  .upload-option:hover .upload-placeholder {
    border-color: var(--accent);
    color: var(--accent);
  }

  .hidden-input {
    display: none;
  }
</style>
