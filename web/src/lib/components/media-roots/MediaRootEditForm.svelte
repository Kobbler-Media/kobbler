<script lang="ts">
  import type { MediaRoot } from '$lib/types';
  import NamingTemplateEditor from './NamingTemplateEditor.svelte';

  interface Props {
    root: MediaRoot;
    saving: boolean;
    editPath: string;
    editName: string;
    editAutoAdd: boolean;
    editWriteSidecars: boolean;
    editWriteImages: boolean;
    editNamingTemplate: string;
    editMaxConcurrentIo: number;
    editScanIntervalSeconds: number | null;
    onEditPathChange: (v: string) => void;
    onEditNameChange: (v: string) => void;
    onEditAutoAddChange: (v: boolean) => void;
    onEditWriteSidecarsChange: (v: boolean) => void;
    onEditWriteImagesChange: (v: boolean) => void;
    onEditNamingTemplateChange: (v: string) => void;
    onEditMaxConcurrentIoChange: (v: number) => void;
    onEditScanIntervalSecondsChange: (v: number | null) => void;
    onBrowsePath: () => void;
  }

  let {
    root,
    saving,
    editPath,
    editName,
    editAutoAdd,
    editWriteSidecars,
    editWriteImages,
    editNamingTemplate,
    editMaxConcurrentIo,
    editScanIntervalSeconds,
    onEditPathChange,
    onEditNameChange,
    onEditAutoAddChange,
    onEditWriteSidecarsChange,
    onEditWriteImagesChange,
    onEditNamingTemplateChange,
    onEditMaxConcurrentIoChange,
    onEditScanIntervalSecondsChange,
    onBrowsePath
  }: Props = $props();
</script>

<div class="edit-form">
  <div class="field-label">
    <span class="label-text">Path</span>
    <div class="path-row">
      <input
        type="text"
        class="field-input"
        value={editPath}
        oninput={(e) => onEditPathChange(e.currentTarget.value)}
        placeholder="/path/to/media"
        disabled={saving}
      />
      <button class="browse-btn" type="button" onclick={onBrowsePath} disabled={saving}>
        Browse
      </button>
    </div>
  </div>

  <label class="field-label">
    <span class="label-text">Name</span>
    <input
      type="text"
      class="field-input"
      value={editName}
      oninput={(e) => onEditNameChange(e.currentTarget.value)}
      placeholder="Optional display name"
      disabled={saving}
    />
  </label>

  <label class="checkbox-label">
    <input
      type="checkbox"
      checked={editAutoAdd}
      onchange={(e) => onEditAutoAddChange(e.currentTarget.checked)}
      disabled={saving}
    />
    Auto-add new media
  </label>

  <label class="checkbox-label">
    <input
      type="checkbox"
      checked={editWriteSidecars}
      onchange={(e) => onEditWriteSidecarsChange(e.currentTarget.checked)}
      disabled={saving}
    />
    Write sidecar files (NFO, metadata.json)
  </label>

  <label class="checkbox-label">
    <input
      type="checkbox"
      checked={editWriteImages}
      onchange={(e) => onEditWriteImagesChange(e.currentTarget.checked)}
      disabled={saving}
    />
    Write image files (posters, fanart)
  </label>

  <label class="field-label">
    <span class="label-text">Max concurrent I/O</span>
    <input
      type="number"
      class="field-input"
      min="1"
      max="16"
      value={editMaxConcurrentIo}
      oninput={(e) => onEditMaxConcurrentIoChange(Number(e.currentTarget.value))}
      disabled={saving}
    />
    <span class="help-text">
      Maximum simultaneous heavy-IO jobs (thumbnails, skip detect, extract) against this media root.
      Lower this if you see IO stalls or job timeouts; raise it on fast local storage. Recommended: <strong
        >1</strong
      >
      for slow NAS over CIFS/NFS, <strong>2–4</strong> for fast NAS, <strong>4+</strong> for local SSD.
      Default: 1.
    </span>
  </label>

  <label class="field-label">
    <span class="label-text">Scan interval (seconds)</span>
    <input
      type="number"
      class="field-input"
      min="60"
      max="86400"
      placeholder="Leave empty for global default"
      value={editScanIntervalSeconds ?? ''}
      oninput={(e) => {
        const raw = e.currentTarget.value.trim();
        onEditScanIntervalSecondsChange(raw === '' ? null : Number(raw));
      }}
      disabled={saving}
    />
    <span class="help-text">
      How often to re-walk this media root's directory tree looking for changes. Lower values detect
      external file changes faster but add CIFS/NFS chatter. New files added via kobload or the
      filesystem watcher are picked up immediately regardless of this setting — this is only a
      safety net for files added outside of kobbler's knowledge. Leave empty to use the global
      default. Min <strong>60</strong>, max <strong>86400</strong> (1 day).
    </span>
  </label>

  <NamingTemplateEditor
    value={editNamingTemplate}
    mediaType={root.media_type}
    disabled={saving}
    onchange={onEditNamingTemplateChange}
  />
</div>

<style>
  .edit-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .field-label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .label-text {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .field-input {
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg);
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
    resize: vertical;
  }

  .path-row {
    display: flex;
    gap: 6px;
    align-items: stretch;
  }

  .path-row .field-input {
    flex: 1;
  }

  .browse-btn {
    padding: 8px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg);
    color: var(--ink);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .browse-btn:hover:not(:disabled) {
    background: var(--ring);
  }

  .browse-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
  }

  .checkbox-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    cursor: pointer;
    accent-color: var(--accent);
  }

  .help-text {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.4;
  }
</style>
