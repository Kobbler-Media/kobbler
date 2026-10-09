<script lang="ts">
  import { goto } from '$app/navigation';
  import {
    getNotificationsState,
    initNotificationsStore,
    markRead,
    markAllRead,
    formatTimeAgo
  } from '$lib/stores/notifications.svelte';
  import { onMount } from 'svelte';

  const notifState = getNotificationsState();
  let dropdownOpen = $state(false);
  let dropdownRef = $state<HTMLDivElement | null>(null);

  onMount(() => {
    initNotificationsStore();

    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef && !dropdownRef.contains(event.target as Node)) {
        dropdownOpen = false;
      }
    }

    document.addEventListener('click', handleClickOutside);

    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  function toggleDropdown(event: MouseEvent) {
    event.stopPropagation();
    const opening = !dropdownOpen;
    dropdownOpen = opening;
    if (opening && notifState.unreadCount > 0) {
      markAllRead().catch(() => {});
    }
  }

  function handleViewAll() {
    dropdownOpen = false;
    goto('/notifications');
  }

  async function handleMarkAllRead() {
    try {
      await markAllRead();
    } catch {
      // ignore
    }
  }

  async function handleNotificationClick(id: string, url: string | null) {
    dropdownOpen = false;
    try {
      await markRead(id);
    } catch {
      // ignore
    }
    if (url) {
      goto(url);
    }
  }

  function getEntityIcon(entityType: string | null): string {
    switch (entityType) {
      case 'book':
        return 'M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 4h5v8l-2.5-1.5L6 12V4z';
      case 'movie':
        return 'M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z';
      case 'episode':
      case 'tv':
        return 'M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12zm-5-6l-7 4V7l7 4z';
      default:
        return 'M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z';
    }
  }
</script>

<div class="notif-indicator" bind:this={dropdownRef}>
  <button
    class="notif-btn"
    type="button"
    aria-label="Notifications"
    onclick={toggleDropdown}
    class:has-unread={notifState.unreadCount > 0}
  >
    <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
      <path
        d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
      ></path>
    </svg>
    {#if notifState.unreadCount > 0}
      <span class="badge">{notifState.unreadCount > 99 ? '99+' : notifState.unreadCount}</span>
    {/if}
  </button>

  {#if dropdownOpen}
    <div class="dropdown">
      <div class="dropdown-header">
        <span class="dropdown-title">Notifications</span>
        {#if notifState.unreadCount > 0}
          <button type="button" class="mark-read-btn" onclick={handleMarkAllRead}>
            Mark all read
          </button>
        {/if}
      </div>

      {#if notifState.notifications.length === 0}
        <div class="empty-state">No notifications yet</div>
      {:else}
        <div class="notif-list">
          {#each notifState.notifications as notif (notif.id)}
            <button
              type="button"
              class="notif-item"
              class:unread={!notif.read}
              onclick={() => handleNotificationClick(notif.id, notif.entity_url)}
            >
              <div class="notif-icon">
                <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
                  <path d={getEntityIcon(notif.entity_type)}></path>
                </svg>
              </div>
              <div class="notif-content">
                <div class="notif-title">{notif.title}</div>
                <div class="notif-message">{notif.message}</div>
                <div class="notif-time">{formatTimeAgo(notif.created_at)}</div>
              </div>
              {#if !notif.read}
                <span class="unread-dot"></span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}

      <button class="view-all-btn" type="button" onclick={handleViewAll}>
        View all notifications
      </button>
    </div>
  {/if}
</div>

<style>
  .notif-indicator {
    position: relative;
  }

  .notif-btn {
    border-radius: 12px;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    position: relative;
    transition: border-color 0.2s ease;
  }

  .notif-btn:hover {
    background: var(--ring);
  }

  .notif-btn.has-unread {
    border-color: var(--accent);
  }

  .badge {
    position: absolute;
    top: -4px;
    right: -4px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--danger, #ef4444);
    color: #fff;
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
    text-align: center;
    pointer-events: none;
  }

  .dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 8px;
    min-width: 320px;
    max-width: 360px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    z-index: 100;
    overflow: hidden;
  }

  .dropdown-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--ring);
  }

  .dropdown-title {
    font-weight: 600;
    font-size: 13px;
  }

  .mark-read-btn {
    background: none;
    border: none;
    color: var(--accent);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
    transition: background 0.15s;
  }

  .mark-read-btn:hover {
    background: var(--accent-dim);
  }

  .empty-state {
    padding: 24px 20px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }

  .notif-list {
    max-height: 340px;
    overflow-y: auto;
  }

  .notif-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 14px;
    border: none;
    border-bottom: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    text-align: left;
    width: 100%;
    font-family: inherit;
    transition: background 0.12s;
  }

  .notif-item:last-child {
    border-bottom: none;
  }

  .notif-item:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .notif-item:hover {
    background: rgba(0, 0, 0, 0.03);
  }

  .notif-item.unread {
    background: var(--accent-dim);
  }

  .notif-item.unread:hover {
    background: rgba(211, 140, 90, 0.12);
  }

  :global([data-theme='light']) .notif-item.unread {
    background: rgba(200, 122, 70, 0.08);
  }

  :global([data-theme='light']) .notif-item.unread:hover {
    background: rgba(200, 122, 70, 0.12);
  }

  .notif-icon {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: rgba(211, 140, 90, 0.12);
    color: var(--accent);
    margin-top: 1px;
  }

  .notif-content {
    flex: 1;
    min-width: 0;
  }

  .notif-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    margin-bottom: 1px;
  }

  .notif-message {
    font-size: 13px;
    font-weight: 500;
    line-height: 1.3;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .notif-time {
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .unread-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    flex-shrink: 0;
    margin-top: 6px;
  }

  .view-all-btn {
    width: 100%;
    padding: 12px;
    border: none;
    border-top: 1px solid var(--ring);
    background: transparent;
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    text-align: center;
    font-family: inherit;
  }

  .view-all-btn:hover {
    background: rgba(211, 140, 90, 0.1);
  }
</style>
