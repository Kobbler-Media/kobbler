<script lang="ts">
  import Cropper from 'svelte-easy-crop';
  import type { CropArea } from 'svelte-easy-crop';

  interface Props {
    currentAvatarUrl?: string;
    onSaved: () => void;
  }

  let { currentAvatarUrl = '', onSaved }: Props = $props();

  let imageSrc = $state('');
  let crop = $state({ x: 0, y: 0 });
  let zoom = $state(1);
  let pixelCrop = $state<CropArea | null>(null);
  let saving = $state(false);
  let error = $state('');
  let fileInput = $state<HTMLInputElement | null>(null);

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;

    if (!file.type.startsWith('image/')) {
      error = 'Please select an image file';
      return;
    }

    error = '';
    const reader = new FileReader();
    reader.onload = () => {
      imageSrc = reader.result as string;
      crop = { x: 0, y: 0 };
      zoom = 1;
    };
    reader.readAsDataURL(file);
  }

  function handleCropComplete(e: { pixels: CropArea }) {
    pixelCrop = e.pixels;
  }

  async function cropAndUpload() {
    if (!imageSrc || !pixelCrop) return;
    saving = true;
    error = '';

    try {
      const canvas = document.createElement('canvas');
      const size = 256;
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext('2d')!;

      const img = new Image();
      img.crossOrigin = 'anonymous';
      await new Promise<void>((resolve, reject) => {
        img.onload = () => resolve();
        img.onerror = () => reject(new Error('Failed to load image'));
        img.src = imageSrc;
      });

      ctx.drawImage(
        img,
        pixelCrop!.x,
        pixelCrop!.y,
        pixelCrop!.width,
        pixelCrop!.height,
        0,
        0,
        size,
        size
      );

      const blob = await new Promise<Blob>((resolve, reject) => {
        canvas.toBlob(
          (b) => (b ? resolve(b) : reject(new Error('Failed to create blob'))),
          'image/jpeg',
          0.85
        );
      });

      const resp = await fetch(`${apiBase}/api/v1/admin/me/avatar`, {
        method: 'PUT',
        headers: { 'Content-Type': 'image/jpeg' },
        body: blob,
        credentials: 'include'
      });

      if (!resp.ok) {
        throw new Error(await resp.text());
      }

      imageSrc = '';
      onSaved();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Upload failed';
    } finally {
      saving = false;
    }
  }

  function cancel() {
    imageSrc = '';
    error = '';
  }
</script>

{#if imageSrc}
  <div class="cropper-container">
    <div class="cropper-area">
      <Cropper
        image={imageSrc}
        bind:crop
        bind:zoom
        aspect={1}
        cropShape="round"
        showGrid={false}
        oncropcomplete={handleCropComplete}
      />
    </div>
    <div class="cropper-controls">
      <label class="zoom-label">
        <span>Zoom</span>
        <input type="range" min="1" max="3" step="0.05" bind:value={zoom} class="zoom-slider" />
      </label>
      <div class="cropper-actions">
        <button type="button" class="btn-cancel" onclick={cancel} disabled={saving}>Cancel</button>
        <button type="button" class="btn-save" onclick={cropAndUpload} disabled={saving}>
          {saving ? 'Saving...' : 'Save'}
        </button>
      </div>
    </div>
  </div>
{:else}
  <button type="button" class="upload-trigger" onclick={() => fileInput?.click()}>
    {#if currentAvatarUrl}
      <img src={currentAvatarUrl} alt="Avatar" class="current-avatar" />
      <span class="upload-overlay">Change</span>
    {:else}
      <span class="upload-placeholder">Upload Avatar</span>
    {/if}
  </button>
  <input
    bind:this={fileInput}
    type="file"
    accept="image/jpeg,image/png,image/webp"
    onchange={handleFileSelect}
    class="hidden-input"
  />
{/if}

{#if error}
  <p class="error-msg">{error}</p>
{/if}

<style>
  .cropper-container {
    width: 100%;
  }

  .cropper-area {
    position: relative;
    width: 100%;
    height: 280px;
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg);
  }

  .cropper-controls {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-top: 12px;
  }

  .zoom-label {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
    color: var(--muted);
  }

  .zoom-slider {
    flex: 1;
    accent-color: var(--accent);
  }

  .cropper-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .btn-cancel,
  .btn-save {
    padding: 6px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: 1px solid var(--ring);
  }

  .btn-cancel {
    background: transparent;
    color: var(--ink);
  }

  .btn-cancel:hover {
    background: var(--ring);
  }

  .btn-save {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

  .btn-save:hover {
    opacity: 0.9;
  }

  .btn-save:disabled,
  .btn-cancel:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .upload-trigger {
    position: relative;
    width: 96px;
    height: 96px;
    border-radius: 50%;
    border: 2px dashed var(--ring);
    background: var(--card);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    transition: border-color 0.15s;
  }

  .upload-trigger:hover {
    border-color: var(--accent);
  }

  .current-avatar {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .upload-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.5);
    color: white;
    font-size: 12px;
    font-weight: 500;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .upload-trigger:hover .upload-overlay {
    opacity: 1;
  }

  .upload-placeholder {
    font-size: 11px;
    color: var(--muted);
    text-align: center;
    padding: 8px;
  }

  .hidden-input {
    display: none;
  }

  .error-msg {
    color: var(--danger);
    font-size: 12px;
    margin-top: 8px;
  }
</style>
