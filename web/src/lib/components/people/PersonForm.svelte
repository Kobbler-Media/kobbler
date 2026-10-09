<script lang="ts">
  import type { PersonDetail } from '$lib/types';
  import { uploadPersonImage, personImageUrl } from '$lib/api/client';

  export interface PersonFormData {
    name: string;
    sort_name?: string | null;
    bio?: string | null;
    birth_date?: string | null;
    death_date?: string | null;
    identifiers?: Record<string, string>;
  }

  interface Props {
    person?: PersonDetail | null;
    onSave: (data: PersonFormData) => Promise<void>;
    onCancel: () => void;
    onDelete?: () => void;
    saving?: boolean;
    deleting?: boolean;
    hideFooter?: boolean;
    requestSave?: () => void;
    canSave?: boolean;
  }

  let {
    person = null,
    onSave,
    onCancel,
    onDelete,
    saving = false,
    deleting = false,
    hideFooter = false,
    requestSave = $bindable(),
    canSave = $bindable()
  }: Props = $props();

  let name = $state('');
  let sort_name = $state('');
  let bio = $state('');
  let birth_date = $state('');
  let death_date = $state('');
  let identifiers = $state<Array<{ key: string; value: string }>>([]);

  $effect(() => {
    if (person) {
      name = person.name || '';
      sort_name = person.sort_name || '';
      bio = person.bio || '';
      birth_date = person.birth_date || '';
      death_date = person.death_date || '';
      identifiers = person.identifiers
        ? Object.entries(person.identifiers).map(([key, value]) => ({
            key,
            value: String(value)
          }))
        : [];
    } else {
      name = '';
      sort_name = '';
      bio = '';
      birth_date = '';
      death_date = '';
      identifiers = [];
    }
  });

  $effect(() => {
    canSave = name.trim().length > 0;
  });

  function handleSave() {
    if (!canSave) return;
    const identifiersObj = identifiers.reduce(
      (acc, { key, value }) => {
        if (key.trim() && value.trim()) {
          acc[key.trim()] = value.trim();
        }
        return acc;
      },
      {} as Record<string, string>
    );

    const formData: PersonFormData = {
      name: name.trim(),
      sort_name: sort_name.trim() || null,
      bio: bio.trim() || null,
      birth_date: birth_date.trim() || null,
      death_date: death_date.trim() || null,
      identifiers: Object.keys(identifiersObj).length > 0 ? identifiersObj : undefined
    };
    onSave(formData);
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

  let imageUploading = $state(false);
  let imageKey = $state(0);

  async function handleImageUpload(e: Event) {
    if (!person) return;
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    imageUploading = true;
    try {
      await uploadPersonImage(person.id, file);
      imageKey++;
    } finally {
      imageUploading = false;
      input.value = '';
    }
  }
</script>

<div class="person-form">
  <div class="form-grid">
    <div class="form-field full-width">
      <label for="name">Name*</label>
      <input id="name" type="text" bind:value={name} placeholder="Enter person name" />
    </div>

    <div class="form-field full-width">
      <label for="sort_name">Sort Name</label>
      <input
        id="sort_name"
        type="text"
        bind:value={sort_name}
        placeholder="e.g. Lastname, Firstname"
      />
    </div>

    <div class="form-field">
      <label for="birth_date">Birth Date</label>
      <input id="birth_date" type="text" bind:value={birth_date} placeholder="YYYY-MM-DD" />
    </div>

    <div class="form-field">
      <label for="death_date">Death Date</label>
      <input id="death_date" type="text" bind:value={death_date} placeholder="YYYY-MM-DD" />
    </div>

    <div class="form-field full-width">
      <label for="bio">Biography</label>
      <textarea id="bio" bind:value={bio} placeholder="Enter biography" rows="4"></textarea>
    </div>

    {#if person}
      <!-- svelte-ignore a11y_label_has_associated_control -->
      <div class="form-field full-width">
        <label>Photo</label>
        <div class="image-field">
          <img
            src="{personImageUrl(person.id)}?v={imageKey}"
            alt="Person"
            class="image-preview"
            onerror={(e) => ((e.target as HTMLImageElement).style.display = 'none')}
          />
          <label class="btn-sec small upload-btn">
            {imageUploading ? 'Uploading...' : 'Upload Photo'}
            <input
              type="file"
              accept="image/*"
              onchange={handleImageUpload}
              disabled={imageUploading}
              hidden
            />
          </label>
        </div>
      </div>
    {/if}

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
  </div>

  {#if !hideFooter}
    <div class="form-footer">
      <div class="footer-left">
        {#if person && onDelete}
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
  .person-form {
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
  input,
  textarea {
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    border-radius: 8px;
    padding: 8px 12px;
    font-size: 14px;
    font-family: inherit;
    transition: border-color 0.2s;
  }
  input:focus,
  textarea:focus {
    outline: none;
    border-color: var(--accent);
  }
  input::placeholder,
  textarea::placeholder {
    color: var(--muted);
    opacity: 0.6;
  }
  textarea {
    resize: vertical;
    min-height: 80px;
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
  .image-field {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .image-preview {
    width: 64px;
    height: 64px;
    object-fit: cover;
    border-radius: 50%;
    border: 1px solid var(--ring);
  }
  .upload-btn {
    cursor: pointer;
    white-space: nowrap;
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
