// Notifications store using Svelte 5 runes — driven by unified SSE events via layout.

import type { UserNotification } from '$lib/types';
import {
  getNotificationInbox,
  getUnreadNotificationCount,
  markNotificationRead,
  markAllNotificationsRead
} from '$lib/api/client';

// State
let notifications = $state.raw<UserNotification[]>([]);
let unreadCount = $state<number>(0);
let total = $state<number>(0);

// Getters
export function getNotificationsState() {
  return {
    get notifications() {
      return notifications;
    },
    get unreadCount() {
      return unreadCount;
    },
    get total() {
      return total;
    }
  };
}

// Load functions
export async function loadNotifications(limit = 20, offset = 0) {
  const response = await getNotificationInbox({ limit, offset });
  notifications = response.notifications;
  total = response.total;
}

export async function loadUnreadCount() {
  unreadCount = await getUnreadNotificationCount();
}

/** Handle SSE notification_created event. Re-fetches count + recent list. */
export function handleNotificationCreated() {
  loadUnreadCount().catch(() => {});
  loadNotifications(10).catch(() => {});
}

/** Mark a single notification as read. */
export async function markRead(id: string) {
  const wasUnread = notifications.some((n) => n.id === id && !n.read);
  await markNotificationRead(id);
  notifications = notifications.map((n) => (n.id === id ? { ...n, read: true } : n));
  if (wasUnread) {
    unreadCount = Math.max(0, unreadCount - 1);
  }
}

/** Mark all notifications as read. */
export async function markAllRead() {
  await markAllNotificationsRead();
  notifications = notifications.map((n) => ({ ...n, read: true }));
  unreadCount = 0;
}

// Initialize — call this when the app loads
export async function initNotificationsStore() {
  try {
    await Promise.all([loadNotifications(10), loadUnreadCount()]);
  } catch {
    // Ignore errors on init — user might not be logged in
  }
}

// Re-export formatTimeAgo from jobs store for shared use
export { formatTimeAgo } from '$lib/stores/jobs.svelte';
