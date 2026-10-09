<script lang="ts">
  import { tick } from 'svelte';

  type MediaType = 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';

  interface TemplateVariable {
    name: string;
    label: string;
    numeric: boolean;
  }

  interface AltSample {
    label: string;
    data: Record<string, string | number>;
  }

  interface Props {
    value: string;
    mediaType: MediaType;
    disabled?: boolean;
    onchange: (value: string) => void;
  }

  let { value, mediaType, disabled = false, onchange }: Props = $props();

  const DEFAULTS: Record<MediaType, string> = {
    tv: '{show_title}/Season {season:02}/{show_title} - S{season:02}E{episode:02} - {episode_title}.{ext}',
    movie: '{title} ({year})/{title} ({year}).{ext}',
    book: '{author}/{title}/{title}.{ext}',
    audiobook: '{author}/{title}/{title}.{ext}',
    sport: '{show_title}/{show_title} - R{episode:02} - {episode_title}.{ext}'
  };

  const VARIABLES: Record<MediaType, TemplateVariable[]> = {
    tv: [
      { name: 'show_title', label: 'Show Title', numeric: false },
      { name: 'season', label: 'Season', numeric: true },
      { name: 'episode', label: 'Episode', numeric: true },
      { name: 'episode_title', label: 'Episode Title', numeric: false },
      { name: 'ext', label: 'Extension', numeric: false }
    ],
    movie: [
      { name: 'title', label: 'Title', numeric: false },
      { name: 'year', label: 'Year', numeric: true },
      { name: 'ext', label: 'Extension', numeric: false }
    ],
    book: [
      { name: 'title', label: 'Title', numeric: false },
      { name: 'author', label: 'Author', numeric: false },
      { name: 'series', label: 'Series', numeric: false },
      { name: 'series_index', label: 'Series #', numeric: true },
      { name: 'ext', label: 'Extension', numeric: false }
    ],
    audiobook: [
      { name: 'title', label: 'Title', numeric: false },
      { name: 'author', label: 'Author', numeric: false },
      { name: 'series', label: 'Series', numeric: false },
      { name: 'series_index', label: 'Series #', numeric: true },
      { name: 'ext', label: 'Extension', numeric: false }
    ],
    sport: [
      { name: 'show_title', label: 'League', numeric: false },
      { name: 'season', label: 'Year', numeric: false },
      { name: 'episode', label: 'Round', numeric: true },
      { name: 'episode_title', label: 'Event', numeric: false },
      { name: 'ext', label: 'Extension', numeric: false }
    ]
  };

  const SAMPLES: Record<MediaType, { label: string; data: Record<string, string | number> }> = {
    tv: {
      label: 'With episode title',
      data: {
        show_title: 'Breaking Bad',
        season: 2,
        episode: 5,
        episode_title: 'Breakage',
        ext: 'mkv'
      }
    },
    movie: {
      label: 'Example',
      data: {
        title: 'Inception',
        year: 2010,
        ext: 'mp4'
      }
    },
    book: {
      label: 'In a series',
      data: {
        title: 'The Way of Kings',
        author: 'Brandon Sanderson',
        series: 'Stormlight Archive',
        series_index: 1,
        ext: 'epub'
      }
    },
    audiobook: {
      label: 'In a series',
      data: {
        title: 'The Way of Kings',
        author: 'Brandon Sanderson',
        series: 'Stormlight Archive',
        series_index: 1,
        ext: 'm4b'
      }
    },
    sport: {
      label: 'With event name',
      data: {
        show_title: 'Premier League',
        season: 2024,
        episode: 12,
        episode_title: 'Arsenal vs Chelsea',
        ext: 'mkv'
      }
    }
  };

  const ALT_SAMPLES: Record<MediaType, AltSample | null> = {
    tv: {
      label: 'Without episode title',
      data: { show_title: 'Lost', season: 1, episode: 3, ext: 'mkv' }
    },
    movie: null,
    book: {
      label: 'Standalone book',
      data: { title: 'Project Hail Mary', author: 'Andy Weir', ext: 'epub' }
    },
    audiobook: {
      label: 'Standalone audiobook',
      data: { title: 'Project Hail Mary', author: 'Andy Weir', ext: 'm4b' }
    },
    sport: {
      label: 'Without event name',
      data: { show_title: 'Formula 1', season: 2024, episode: 5, ext: 'mkv' }
    }
  };

  let textareaEl: HTMLTextAreaElement | undefined = $state();

  const isUsingDefault = $derived(value === '');
  const defaultTemplate = $derived(DEFAULTS[mediaType]);
  const variables = $derived(VARIABLES[mediaType]);
  const displayText = $derived(isUsingDefault ? defaultTemplate : value);

  function renderTemplate(template: string, data: Record<string, string | number>): string {
    return template.replace(/\{([^}]+)\}/g, (match, placeholder: string) => {
      const [field, fmtSpec] = placeholder.split(':');
      const val = data[field];
      if (val === undefined) return match;
      if (fmtSpec && fmtSpec.startsWith('0')) {
        const width = parseInt(fmtSpec.slice(1), 10);
        return String(val).padStart(width, '0');
      }
      return String(val);
    });
  }

  /** Strip unresolved {…} from a string and clean up separators. */
  function stripUnresolved(s: string): string {
    return s
      .replace(/\{[^}]+\}/g, '')
      .replace(/\s+/g, ' ')
      .replace(/ - \./g, '.')
      .replace(/ \./g, '.')
      .trim()
      .replace(/^- /, '')
      .replace(/ -$/, '');
  }

  /** Render template with data and build cleaned path segments. */
  function buildSegments(template: string, data: Record<string, string | number>): string[] {
    const rendered = renderTemplate(template, data);
    const hasUnresolved = (s: string) => /\{[^}]+\}/.test(s);
    const result: string[] = [];
    for (const seg of rendered.split('/')) {
      if (hasUnresolved(seg)) {
        const cleaned = stripUnresolved(seg);
        if (cleaned) result.push(cleaned);
      } else {
        result.push(seg);
      }
    }
    return result;
  }

  const primarySample = $derived(SAMPLES[mediaType]);
  const altSample = $derived(ALT_SAMPLES[mediaType]);
  const primarySegments = $derived(buildSegments(displayText, primarySample.data));
  const altSegments = $derived(altSample ? buildSegments(displayText, altSample.data) : null);

  function emit(text: string) {
    if (text === defaultTemplate || text === '') {
      onchange('');
    } else {
      onchange(text);
    }
  }

  function handleInput(e: Event) {
    emit((e.target as HTMLTextAreaElement).value);
  }

  function resetToDefault() {
    onchange('');
  }

  function insertVariable(variable: TemplateVariable) {
    if (disabled) return;
    const token = variable.numeric ? `{${variable.name}:02}` : `{${variable.name}}`;
    const currentText = isUsingDefault ? defaultTemplate : value;
    const pos = textareaEl?.selectionStart ?? currentText.length;
    const before = currentText.slice(0, pos);
    const after = currentText.slice(pos);
    emit(before + token + after);

    tick().then(() => {
      const newPos = pos + token.length;
      textareaEl?.setSelectionRange(newPos, newPos);
      textareaEl?.focus();
    });
  }
</script>

{#snippet pathPreview(segments: string[])}
  <div class="preview-path">
    {#each segments as segment, i (i)}
      {#if i > 0}
        <span class="sep">/</span>
      {/if}
      <span class={i === segments.length - 1 ? 'seg file' : 'seg folder'}>
        {#if i < segments.length - 1}
          <svg class="folder-icon" viewBox="0 0 16 16" width="12" height="12" fill="currentColor">
            <path
              d="M1.75 1A1.75 1.75 0 0 0 0 2.75v10.5C0 14.216.784 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-8.5A1.75 1.75 0 0 0 14.25 3H7.5a.25.25 0 0 1-.2-.1l-.9-1.2C6.07 1.26 5.55 1 5 1H1.75Z"
            ></path>
          </svg>
        {/if}
        {segment}
      </span>
    {/each}
  </div>
{/snippet}

<div class="template-editor">
  <div class="template-header">
    <span class="template-label">Naming Template</span>
    {#if isUsingDefault}
      <span class="default-badge">default</span>
    {:else}
      <button class="reset-btn" type="button" onclick={resetToDefault} {disabled}>
        Reset to default
      </button>
    {/if}
  </div>

  <div class="textarea-wrap" class:is-default={isUsingDefault}>
    <textarea
      bind:this={textareaEl}
      class="template-input"
      value={displayText}
      oninput={handleInput}
      {disabled}
      rows={2}
      spellcheck={false}></textarea>
  </div>

  <div class="chips-row">
    <span class="chips-label">Variables</span>
    <div class="chips-wrap">
      {#each variables as variable (variable.name)}
        <button
          class="var-chip"
          type="button"
          onclick={() => insertVariable(variable)}
          {disabled}
          title={variable.numeric ? `Insert {${variable.name}:02}` : `Insert {${variable.name}}`}
        >
          <span class="var-chip-name">{variable.label}</span>
          {#if variable.numeric}
            <span class="var-chip-fmt">02</span>
          {/if}
        </button>
      {/each}
    </div>
  </div>

  <div class="preview-section">
    <span class="preview-label">Preview</span>
    <div class="preview-row">
      {#if altSegments}
        <span class="preview-scenario">{primarySample.label}</span>
      {/if}
      {@render pathPreview(primarySegments)}
    </div>
    {#if altSegments}
      <div class="preview-row">
        <span class="preview-scenario">{altSample?.label}</span>
        {@render pathPreview(altSegments)}
      </div>
    {/if}
  </div>
</div>

<style>
  .template-editor {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .template-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .template-label {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .default-badge {
    font-size: 10px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
    letter-spacing: 0.3px;
    text-transform: uppercase;
  }

  .reset-btn {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .reset-btn:hover:not(:disabled) {
    color: var(--accent);
    border-color: var(--accent);
  }

  .reset-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  /* Textarea */
  .textarea-wrap {
    position: relative;
  }

  .textarea-wrap.is-default .template-input:not(:focus) {
    color: var(--muted);
  }

  .template-input {
    width: 100%;
    box-sizing: border-box;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg);
    color: var(--ink);
    font-size: 12px;
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'JetBrains Mono', monospace;
    line-height: 1.6;
    resize: vertical;
    transition: border-color 0.15s ease;
  }

  .template-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .template-input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Variable chips */
  .chips-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .chips-label {
    font-size: 10px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.4px;
    flex-shrink: 0;
    opacity: 0.7;
  }

  .chips-wrap {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }

  .var-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    border-radius: 999px;
    background: var(--bg-2, rgba(255, 255, 255, 0.05));
    border: 1px solid var(--ring);
    font-size: 11px;
    color: var(--ink);
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .var-chip:hover:not(:disabled) {
    background: var(--accent);
    color: #1d130c;
    border-color: var(--accent);
    transform: translateY(-1px);
  }

  .var-chip:active:not(:disabled) {
    transform: translateY(0);
  }

  .var-chip:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .var-chip-name {
    font-weight: 500;
  }

  .var-chip-fmt {
    font-size: 9px;
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
    padding: 0 4px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
    line-height: 1.4;
  }

  .var-chip:hover:not(:disabled) .var-chip-fmt {
    background: rgba(0, 0, 0, 0.15);
    color: inherit;
  }

  /* Preview */
  .preview-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .preview-label {
    font-size: 10px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.4px;
    opacity: 0.7;
  }

  .preview-row {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .preview-scenario {
    font-size: 10px;
    color: var(--muted);
    opacity: 0.7;
  }

  .preview-path {
    font-size: 12px;
    font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'JetBrains Mono', monospace;
    padding: 8px 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    line-height: 1.6;
    word-break: break-all;
  }

  .sep {
    color: var(--ring);
    padding: 0 1px;
  }

  .seg {
    display: inline-flex;
    align-items: baseline;
    gap: 3px;
  }

  .folder {
    color: var(--muted);
  }

  .folder-icon {
    position: relative;
    top: 2px;
    opacity: 0.4;
    flex-shrink: 0;
  }

  .file {
    color: var(--accent);
    font-weight: 600;
  }
</style>
