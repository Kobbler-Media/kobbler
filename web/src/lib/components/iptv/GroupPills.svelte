<script lang="ts">
  import type { IptvGroup } from '$lib/types';

  interface Props {
    groups: IptvGroup[];
    selected: string | null;
    onselect: (groupId: string | null) => void;
    favouriteGroupIds?: string[];
    onfavouritetoggle?: (groupId: string, isFav: boolean) => void;
  }

  let { groups, selected, onselect, favouriteGroupIds = [], onfavouritetoggle }: Props = $props();

  function isFavourite(groupId: string): boolean {
    return favouriteGroupIds.includes(groupId);
  }

  function handleFavClick(e: Event, groupId: string) {
    e.stopPropagation();
    onfavouritetoggle?.(groupId, !isFavourite(groupId));
  }
</script>

<div class="group-pills">
  <div class="pills-scroll">
    <button
      class="pill"
      class:active={selected === null}
      type="button"
      onclick={() => onselect(null)}
    >
      All
    </button>
    {#each groups as group (group.id)}
      <div class="pill-wrapper">
        <button
          class="pill"
          class:active={selected === group.id}
          type="button"
          onclick={() => onselect(group.id)}
        >
          {#if group.icon}
            <img src={group.icon} alt="" class="pill-icon" />
          {/if}
          {group.name}
          {#if group.channel_count > 0}
            <span class="pill-count">{group.channel_count}</span>
          {/if}
        </button>
        {#if onfavouritetoggle}
          <button
            class="pill-fav"
            class:active={isFavourite(group.id)}
            type="button"
            onclick={(e) => handleFavClick(e, group.id)}
            title={isFavourite(group.id) ? 'Remove from favourites' : 'Add to favourites'}
          >
            <svg
              viewBox="0 0 24 24"
              width="12"
              height="12"
              fill={isFavourite(group.id) ? 'currentColor' : 'none'}
              stroke="currentColor"
              stroke-width="2"
            >
              <path
                d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"
              ></path>
            </svg>
          </button>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .group-pills {
    overflow: hidden;
  }

  .pills-scroll {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    padding: 2px 0;
    scrollbar-width: none;
  }

  .pills-scroll::-webkit-scrollbar {
    display: none;
  }

  .pill-wrapper {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }

  .pill {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border-radius: 20px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    cursor: pointer;
    transition: all 0.15s;
    flex-shrink: 0;
  }

  .pill:hover {
    border-color: var(--accent);
  }

  .pill.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
  }

  .pill-icon {
    width: 16px;
    height: 16px;
    border-radius: 3px;
    object-fit: contain;
  }

  .pill-count {
    font-size: 11px;
    opacity: 0.7;
  }

  .pill-fav {
    display: flex;
    align-items: center;
    padding: 0;
    border: none;
    background: none;
    color: var(--ink-2);
    cursor: pointer;
    opacity: 0.5;
    transition: all 0.15s;
  }

  .pill-fav:hover,
  .pill-fav.active {
    opacity: 1;
    color: #ef4444;
  }
</style>
