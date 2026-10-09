<script lang="ts">
  import { onMount } from 'svelte';
  import type { PersonDetail, PersonFilmographyCredit } from '$lib/types';
  import { getPersonFilmography, wantPersonCredit, dismissPersonCredit } from '$lib/api/client';
  import { isActivelyWanted } from '$lib/utils/want-status';
  import { showMessage } from '$lib/context/app.svelte';

  interface Props {
    person: PersonDetail;
    onEdit?: () => void;
    onDelete?: () => void;
    onViewFullPage?: () => void;
    onCreditClick?: (libraryId: string, mediaType: string) => void;
  }

  let { person, onEdit, onDelete, onViewFullPage, onCreditClick }: Props = $props();

  let credits = $state<PersonFilmographyCredit[]>([]);
  let loadingCredits = $state(false);
  const MAX_CREDITS = 20;

  async function loadFilmography() {
    loadingCredits = true;
    try {
      const result = await getPersonFilmography(person.id);
      credits = result.credits.filter((c) => !c.dismissed);
    } catch {
      // Non-critical
    } finally {
      loadingCredits = false;
    }
  }

  async function handleWant(credit: PersonFilmographyCredit) {
    try {
      await wantPersonCredit(person.id, credit.provider_id, credit.media_type);
      showMessage(`Added "${credit.title}" to wanted`);
      await loadFilmography();
    } catch {
      showMessage('Failed to want credit');
    }
  }

  async function handleDismiss(credit: PersonFilmographyCredit) {
    try {
      await dismissPersonCredit(person.id, credit.provider_id, credit.media_type);
      showMessage(`Dismissed "${credit.title}"`);
      await loadFilmography();
    } catch {
      showMessage('Failed to dismiss credit');
    }
  }

  onMount(() => {
    loadFilmography();
  });

  $effect(() => {
    person.id;
    loadFilmography();
  });

  const displayCredits = $derived(credits.slice(0, MAX_CREDITS));
  const hasMore = $derived(credits.length > MAX_CREDITS);
</script>

<div class="person-detail">
  {#if person.image_url}
    <div class="photo-section">
      <img class="person-photo" src={person.image_url} alt={person.name} />
    </div>
  {/if}

  <div class="header-section">
    {#if person.birth_date || person.death_date}
      <div class="dates">
        {#if person.birth_date}Born: {person.birth_date}{/if}
        {#if person.birth_date && person.death_date}
          &middot;
        {/if}
        {#if person.death_date}Died: {person.death_date}{/if}
      </div>
    {/if}
  </div>

  {#if person.bio}
    <div class="section">
      <h4 class="section-title">Biography</h4>
      <p class="bio">{person.bio}</p>
    </div>
  {/if}

  <!-- Filmography -->
  <div class="section">
    <h4 class="section-title">Filmography</h4>
    {#if loadingCredits && credits.length === 0}
      <p class="muted-text">Loading...</p>
    {:else if credits.length === 0}
      <p class="muted-text">No credits found</p>
    {:else}
      <div class="credits-list">
        {#each displayCredits as credit (credit.provider_id + credit.media_type)}
          <div class="credit-item">
            {#if credit.in_library && credit.library_id && onCreditClick}
              <button
                class="credit-info clickable"
                type="button"
                onclick={() => onCreditClick!(credit.library_id!, credit.media_type)}
              >
                <span class="credit-type-icon material-icons-outlined">
                  {credit.media_type === 'movie' ? 'movie' : 'tv'}
                </span>
                <div class="credit-text">
                  <span class="credit-title">{credit.title}</span>
                  <span class="credit-role">
                    {credit.character || credit.job || credit.department || ''}
                    {#if credit.year}
                      <span class="credit-year">{credit.year}</span>
                    {/if}
                  </span>
                </div>
              </button>
            {:else}
              <div class="credit-info">
                <span class="credit-type-icon material-icons-outlined">
                  {credit.media_type === 'movie' ? 'movie' : 'tv'}
                </span>
                <div class="credit-text">
                  <span class="credit-title">{credit.title}</span>
                  <span class="credit-role">
                    {credit.character || credit.job || credit.department || ''}
                    {#if credit.year}
                      <span class="credit-year">{credit.year}</span>
                    {/if}
                  </span>
                </div>
              </div>
            {/if}

            <div class="credit-actions">
              {#if credit.in_library}
                <span class="status-badge in-library">
                  <span class="material-icons-outlined" style="font-size: 12px;">check_circle</span>
                </span>
              {:else if isActivelyWanted(credit.want_status)}
                <span class="status-badge wanted">
                  <span class="material-icons-outlined" style="font-size: 12px;">bookmark</span>
                </span>
              {:else}
                <button
                  class="action-btn"
                  type="button"
                  title="Want"
                  onclick={() => handleWant(credit)}
                >
                  <span class="material-icons-outlined" style="font-size: 14px;"
                    >add_circle_outline</span
                  >
                </button>
                <button
                  class="action-btn dismiss-btn"
                  type="button"
                  title="Dismiss"
                  onclick={() => handleDismiss(credit)}
                >
                  <span class="material-icons-outlined" style="font-size: 14px;">close</span>
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
      {#if hasMore && onViewFullPage}
        <button class="view-all-btn" type="button" onclick={onViewFullPage}>
          View all {credits.length} credits
        </button>
      {/if}
    {/if}
  </div>

  {#if onEdit || onDelete}
    <div class="panel-actions">
      {#if onEdit}
        <button class="btn btn-sec" type="button" onclick={onEdit}>Edit</button>
      {/if}
      {#if onDelete}
        <button class="btn btn-danger" type="button" onclick={onDelete}>Delete</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .person-detail {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .photo-section {
    display: flex;
    justify-content: center;
    padding: 0 0 16px;
  }

  .person-photo {
    width: 120px;
    height: 120px;
    border-radius: 50%;
    object-fit: cover;
    border: 3px solid var(--ring);
  }

  .header-section {
    padding: 0 0 8px;
  }

  .dates {
    font-size: 12px;
    color: var(--muted);
  }

  .section {
    padding: 16px 0;
    border-top: 1px solid var(--ring);
  }

  .section-title {
    margin: 0 0 10px;
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
  }

  .bio {
    font-size: 13px;
    line-height: 1.6;
    color: var(--ink);
    margin: 0;
  }

  .muted-text {
    font-size: 13px;
    color: var(--muted);
    margin: 0;
  }

  .credits-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .credit-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 4px;
    border-radius: 6px;
  }

  .credit-item:hover {
    background: var(--bg-2);
  }

  .credit-info {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
  }

  .credit-info.clickable {
    cursor: pointer;
    border: none;
    background: transparent;
    padding: 0;
    font: inherit;
    text-align: left;
    color: inherit;
  }

  .credit-info.clickable:hover .credit-title {
    color: var(--accent);
  }

  .credit-type-icon {
    font-size: 16px;
    color: var(--muted);
    flex-shrink: 0;
  }

  .credit-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .credit-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    transition: color 0.15s;
  }

  .credit-role {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .credit-year {
    opacity: 0.7;
  }

  .credit-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }

  .status-badge {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
  }

  .status-badge.in-library {
    color: var(--success, #22c55e);
  }

  .status-badge.wanted {
    color: var(--wanted, #f59e0b);
  }

  .action-btn {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    opacity: 0;
  }

  .credit-item:hover .action-btn {
    opacity: 1;
  }

  .action-btn:hover {
    background: var(--bg-2);
    color: var(--accent);
  }

  .dismiss-btn:hover {
    color: var(--error, #ef4444);
  }

  .view-all-btn {
    display: block;
    width: 100%;
    padding: 8px;
    margin-top: 8px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--accent);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
    text-align: center;
  }

  .view-all-btn:hover {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .panel-actions {
    display: flex;
    gap: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }
</style>
