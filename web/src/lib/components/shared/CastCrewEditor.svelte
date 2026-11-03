<script lang="ts">
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import { listPeople, personImageUrl, type CastInput, type CrewInput } from '#lib/api/client.js';

  export interface CastEntry {
    person_id: string;
    person_name: string;
    person_image_url: string | null;
    character_name: string;
  }

  export interface CrewEntry {
    person_id: string;
    person_name: string;
    department: string;
    job: string;
  }

  interface Props {
    cast?: CastEntry[];
    crew?: CrewEntry[];
    onCastChange?: (cast: CastInput[]) => void;
    onCrewChange?: (crew: CrewInput[]) => void;
  }

  let { cast = $bindable([]), crew = $bindable([]), onCastChange, onCrewChange }: Props = $props();

  let dragIndex = $state<number | null>(null);
  let dropIndex = $state<number | null>(null);

  async function searchPeople(query: string): Promise<SearchSelectItem[]> {
    const resp = await listPeople({ q: query, limit: 20 });
    return resp.items.map((p) => ({
      value: p.id,
      label: p.name,
      description: p.sort_name || undefined
    }));
  }

  function addCastMember() {
    cast = [
      ...cast,
      {
        person_id: '',
        person_name: '',
        person_image_url: null,
        character_name: ''
      }
    ];
  }

  function removeCastMember(index: number) {
    cast = cast.filter((_, i) => i !== index);
    emitCastChange();
  }

  function handleCastPersonChange(index: number, value: string, item?: SearchSelectItem) {
    cast[index].person_id = value;
    cast[index].person_name = item?.label ?? '';
    cast[index].person_image_url = value ? personImageUrl(value) : null;
    emitCastChange();
  }

  function emitCastChange() {
    onCastChange?.(
      cast
        .filter((c) => c.person_id)
        .map((c, i) => ({
          person_id: c.person_id,
          character_name: c.character_name || null,
          cast_order: i
        }))
    );
  }

  // Drag and drop handlers for cast reordering
  function handleDragStart(index: number) {
    dragIndex = index;
  }

  function handleDragOver(e: DragEvent, index: number) {
    e.preventDefault();
    dropIndex = index;
  }

  function handleDragEnd() {
    if (dragIndex != null && dropIndex != null && dragIndex !== dropIndex) {
      const updated = [...cast];
      const [moved] = updated.splice(dragIndex, 1);
      updated.splice(dropIndex, 0, moved);
      cast = updated;
      emitCastChange();
    }
    dragIndex = null;
    dropIndex = null;
  }

  function addCrewMember() {
    crew = [...crew, { person_id: '', person_name: '', department: '', job: '' }];
  }

  function removeCrewMember(index: number) {
    crew = crew.filter((_, i) => i !== index);
    emitCrewChange();
  }

  function handleCrewPersonChange(index: number, value: string, item?: SearchSelectItem) {
    crew[index].person_id = value;
    crew[index].person_name = item?.label ?? '';
    emitCrewChange();
  }

  function emitCrewChange() {
    onCrewChange?.(
      crew
        .filter((c) => c.person_id && c.department && c.job)
        .map((c) => ({
          person_id: c.person_id,
          department: c.department,
          job: c.job
        }))
    );
  }
</script>

<div class="cast-crew-editor">
  <section class="cast-section">
    <div class="section-header">
      <span>Cast</span>
      <button type="button" class="btn-sec small" onclick={addCastMember}>Add Cast Member</button>
    </div>
    {#each cast as member, i (i)}
      <div
        class="member-row cast-row"
        class:dragging={dragIndex === i}
        class:drop-target={dropIndex === i && dragIndex !== i}
        draggable="true"
        ondragstart={() => handleDragStart(i)}
        ondragover={(e) => handleDragOver(e, i)}
        ondragend={handleDragEnd}
        role="listitem"
      >
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="drag-handle" onmousedown={(e) => e.stopPropagation()}>
          <span class="material-icons-outlined">drag_indicator</span>
        </span>
        <div class="person-select">
          <SearchSelect
            placeholder="Search person..."
            value={member.person_id}
            initialLabel={member.person_name}
            searchFn={searchPeople}
            onchange={(value, item) => handleCastPersonChange(i, value, item)}
          />
        </div>
        <input
          type="text"
          bind:value={member.character_name}
          placeholder="Character name"
          oninput={emitCastChange}
          class="character-input"
        />
        <button
          type="button"
          class="btn-sec small remove-btn"
          onclick={() => removeCastMember(i)}
          aria-label="Remove cast member"
        >
          &times;
        </button>
      </div>
    {/each}
    {#if cast.length === 0}
      <p class="empty-hint">No cast members. Click "Add Cast Member" to add one.</p>
    {/if}
  </section>

  <section class="crew-section">
    <div class="section-header">
      <span>Crew</span>
      <button type="button" class="btn-sec small" onclick={addCrewMember}>Add Crew Member</button>
    </div>
    {#each crew as member, i (i)}
      <div class="member-row crew-row">
        <div class="person-select">
          <SearchSelect
            placeholder="Search person..."
            value={member.person_id}
            initialLabel={member.person_name}
            searchFn={searchPeople}
            onchange={(value, item) => handleCrewPersonChange(i, value, item)}
          />
        </div>
        <input
          type="text"
          bind:value={member.department}
          placeholder="Department"
          oninput={emitCrewChange}
          class="dept-input"
        />
        <input
          type="text"
          bind:value={member.job}
          placeholder="Job"
          oninput={emitCrewChange}
          class="job-input"
        />
        <button
          type="button"
          class="btn-sec small remove-btn"
          onclick={() => removeCrewMember(i)}
          aria-label="Remove crew member"
        >
          &times;
        </button>
      </div>
    {/each}
    {#if crew.length === 0}
      <p class="empty-hint">No crew members. Click "Add Crew Member" to add one.</p>
    {/if}
  </section>
</div>

<style>
  .cast-crew-editor {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.5px;
  }

  .member-row {
    display: grid;
    gap: 8px;
    align-items: center;
    margin-bottom: 8px;
  }

  .cast-row {
    grid-template-columns: auto 1fr 1fr auto;
    transition: opacity 0.15s;
  }

  .cast-row.dragging {
    opacity: 0.4;
  }

  .cast-row.drop-target {
    border-top: 2px solid var(--accent);
  }

  .crew-row {
    grid-template-columns: 1fr 1fr 1fr auto;
  }

  .drag-handle {
    display: flex;
    align-items: center;
    cursor: grab;
    color: var(--muted);
    font-size: 18px;
    padding: 0 2px;
    user-select: none;
  }

  .drag-handle:active {
    cursor: grabbing;
  }

  .person-select {
    min-width: 0;
  }

  input {
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    border-radius: 8px;
    padding: 8px 12px;
    font-size: 14px;
    font-family: inherit;
    min-width: 0;
  }

  input:focus {
    outline: none;
    border-color: var(--accent);
  }

  input::placeholder {
    color: var(--muted);
    opacity: 0.6;
  }

  .remove-btn {
    padding: 6px 10px;
    font-size: 16px;
    line-height: 1;
  }

  .empty-hint {
    font-size: 13px;
    color: var(--muted);
    font-style: italic;
  }
</style>
