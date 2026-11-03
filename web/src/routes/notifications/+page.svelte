<script lang="ts">
  import type { UserNotification } from '#lib/types.js';
  import { goto } from '$app/navigation';
  import { getNotificationInbox } from '#lib/api/client.js';
  import {
    markRead,
    markAllRead,
    loadUnreadCount,
    formatTimeAgo
  } from '#lib/stores/notifications.svelte.js';
  import { onMount } from 'svelte';
  import { on, off, onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';

  let notifications = $state<UserNotification[]>([]);
  let total = $state(0);
  let loading = $state(true);
  let offset = $state(0);
  let unreadOnly = $state(false);
  const limit = 20;

  async function load() {
    loading = true;
    try {
      const resp = await getNotificationInbox({ limit, offset, unread_only: unreadOnly });
      notifications = resp.notifications;
      total = resp.total;
    } catch {
      // ignore
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    markAllRead()
      .catch(() => {})
      .finally(() => load());
    const reconnectCb = onReconnect(() => load());
    const sseCb = on('notification_created', () => load());
    return () => {
      offReconnect(reconnectCb);
      off('notification_created', sseCb);
    };
  });

  function handleFilterChange(unread: boolean) {
    unreadOnly = unread;
    offset = 0;
    load();
  }

  function handlePrev() {
    offset = Math.max(0, offset - limit);
    load();
  }

  function handleNext() {
    offset = offset + limit;
    load();
  }

  async function handleClick(notif: UserNotification) {
    if (!notif.read) {
      try {
        await markRead(notif.id);
        notifications = notifications.map((n) => (n.id === notif.id ? { ...n, read: true } : n));
        // The store only decrements the badge for items in its 10-item cache; this page lists
        // 20, so re-sync the count from the server for clicks beyond that window.
        loadUnreadCount().catch(() => {});
      } catch {
        // ignore
      }
    }
    if (notif.entity_url) {
      goto(notif.entity_url);
    }
  }

  async function handleMarkAllRead() {
    try {
      await markAllRead();
      notifications = notifications.map((n) => ({ ...n, read: true }));
    } catch {
      // ignore
    }
  }

  function getEntityIcon(entityType: string | null): string {
    switch (entityType) {
      case 'book':
        return 'M18 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 4h5v8l-2.5-1.5L6 12V4z';
      case 'movie':
        return 'M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z';
      case 'episode':
      case 'season':
      case 'show':
      case 'tv':
        return 'M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12zm-5-6l-7 4V7l7 4z';
      default:
        return 'M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z';
    }
  }

  const hasMore = $derived(offset + limit < total);
  const hasPrev = $derived(offset > 0);
  const pageNum = $derived(Math.floor(offset / limit) + 1);
  const totalPages = $derived(Math.max(1, Math.ceil(total / limit)));
</script>

<svelte:head>
  <title>Notifications — Kobbler</title>
</svelte:head>

<div class="notifications-page page-container">
  <div class="page-header">
    <h1 class="page-title">Notifications</h1>
    <div class="header-actions">
      <div class="filter-tabs">
        <button
          type="button"
          class="tab"
          class:active={!unreadOnly}
          onclick={() => handleFilterChange(false)}
        >
          All
        </button>
        <button
          type="button"
          class="tab"
          class:active={unreadOnly}
          onclick={() => handleFilterChange(true)}
        >
          Unread
        </button>
      </div>
      <button type="button" class="btn-mark-all" onclick={handleMarkAllRead}>
        Mark all read
      </button>
    </div>
  </div>

  {#if loading && notifications.length === 0}
    <div class="empty-state">Loading...</div>
  {:else if notifications.length === 0}
    <div class="empty-state">
      {unreadOnly ? 'No unread notifications' : 'No notifications yet'}
    </div>
  {:else}
    <div class="notif-list">
      {#each notifications as notif (notif.id)}
        <button
          type="button"
          class="notif-row"
          class:unread={!notif.read}
          onclick={() => handleClick(notif)}
        >
          <div class="notif-icon">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
              <path d={getEntityIcon(notif.entity_type)}></path>
            </svg>
          </div>
          <div class="notif-body">
            <div class="notif-title">{notif.title}</div>
            <div class="notif-message">{notif.message}</div>
          </div>
          <div class="notif-meta">
            <span class="notif-time">{formatTimeAgo(notif.created_at)}</span>
            {#if !notif.read}
              <span class="unread-dot"></span>
            {/if}
          </div>
        </button>
      {/each}
    </div>

    {#if totalPages > 1}
      <div class="pagination">
        <button type="button" class="page-btn" disabled={!hasPrev} onclick={handlePrev}>
          Previous
        </button>
        <span class="page-info">{pageNum} / {totalPages}</span>
        <button type="button" class="page-btn" disabled={!hasMore} onclick={handleNext}>
          Next
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .notifications-page {
    max-width: inherit;
  }

  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
    flex-wrap: wrap;
    gap: 12px;
  }

  .page-title {
    font-size: 22px;
    font-weight: 600;
    margin: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .filter-tabs {
    display: flex;
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
  }

  .tab {
    padding: 6px 14px;
    border: none;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .tab:not(:last-child) {
    border-right: 1px solid var(--ring);
  }

  .tab.active {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .tab:hover:not(.active) {
    color: var(--ink);
  }

  .btn-mark-all {
    padding: 6px 14px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--accent);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.15s;
  }

  .btn-mark-all:hover {
    background: var(--accent-dim);
  }

  .empty-state {
    text-align: center;
    padding: 48px 20px;
    color: var(--muted);
    font-size: 14px;
  }

  .notif-list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .notif-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 14px 16px;
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

  .notif-row:last-child {
    border-bottom: none;
  }

  .notif-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .notif-row:hover {
    background: rgba(0, 0, 0, 0.03);
  }

  .notif-row.unread {
    background: var(--accent-dim);
  }

  .notif-row.unread:hover {
    background: rgba(211, 140, 90, 0.12);
  }

  :global([data-theme='light']) .notif-row.unread {
    background: rgba(200, 122, 70, 0.08);
  }

  :global([data-theme='light']) .notif-row.unread:hover {
    background: rgba(200, 122, 70, 0.12);
  }

  .notif-icon {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: rgba(211, 140, 90, 0.12);
    color: var(--accent);
    margin-top: 1px;
  }

  .notif-body {
    flex: 1;
    min-width: 0;
  }

  .notif-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    margin-bottom: 2px;
  }

  .notif-message {
    font-size: 14px;
    font-weight: 500;
    line-height: 1.4;
  }

  .notif-meta {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 6px;
    flex-shrink: 0;
  }

  .notif-time {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
  }

  .unread-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
  }

  .pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 16px;
    margin-top: 16px;
  }

  .page-btn {
    padding: 6px 14px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.15s;
  }

  .page-btn:hover:not(:disabled) {
    background: var(--ring);
  }

  .page-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .page-info {
    font-size: 12px;
    color: var(--muted);
  }

  @media (max-width: 768px) {
    .notifications-page {
      padding: 16px;
    }

    .page-header {
      flex-direction: column;
      align-items: flex-start;
    }

    .notif-row {
      padding: 12px;
    }
  }
</style>
