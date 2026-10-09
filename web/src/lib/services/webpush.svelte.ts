// Browser push subscription mechanics, shared by the per-user toggle on the
// profile page and the admin agent controls in settings.

import { getVapidKey, subscribeWebPush, unsubscribeWebPush } from '$lib/api/client';

function urlBase64ToUint8Array(base64String: string): Uint8Array {
  const padding = '='.repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, '+').replace(/_/g, '/');
  const rawData = window.atob(base64);
  const outputArray = new Uint8Array(rawData.length);
  for (let i = 0; i < rawData.length; ++i) {
    outputArray[i] = rawData.charCodeAt(i);
  }
  return outputArray;
}

function uint8ArrayToBase64Url(arr: Uint8Array): string {
  return btoa(String.fromCharCode(...arr))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}

export function createWebPushController() {
  let supported = $state(false);
  let subscribed = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function refresh() {
    supported = 'serviceWorker' in navigator && 'PushManager' in window;
    if (!supported) return;
    try {
      const reg = await navigator.serviceWorker.ready;
      subscribed = (await reg.pushManager.getSubscription()) != null;
    } catch {
      subscribed = false;
    }
  }

  async function subscribe(): Promise<boolean> {
    loading = true;
    error = null;
    try {
      // Waiting on serviceWorker.ready hangs forever if registration failed.
      const reg = await Promise.race([
        navigator.serviceWorker.ready,
        new Promise<never>((_, reject) =>
          setTimeout(
            () =>
              reject(new Error('Service worker not available — is the site served over HTTPS?')),
            5000
          )
        )
      ]);

      if ('Notification' in window && Notification.permission === 'denied') {
        error = 'Notification permission is blocked. Reset it in your browser settings.';
        return false;
      }

      const vapidPublicKey = await getVapidKey();
      const sub = await reg.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(vapidPublicKey).buffer as ArrayBuffer
      });
      await subscribeWebPush({
        endpoint: sub.endpoint,
        p256dh: uint8ArrayToBase64Url(new Uint8Array(sub.getKey('p256dh')!)),
        auth: uint8ArrayToBase64Url(new Uint8Array(sub.getKey('auth')!)),
        user_agent: navigator.userAgent
      });
      subscribed = true;
      return true;
    } catch (e) {
      console.error('[webpush] Subscribe failed:', e);
      error =
        e instanceof DOMException && e.name === 'NotAllowedError'
          ? 'Notification permission denied. Reset it in your browser settings.'
          : e instanceof Error
            ? e.message
            : 'Failed to subscribe';
      return false;
    } finally {
      loading = false;
    }
  }

  async function unsubscribe(): Promise<boolean> {
    loading = true;
    error = null;
    try {
      const reg = await navigator.serviceWorker.ready;
      const sub = await reg.pushManager.getSubscription();
      if (sub) {
        await unsubscribeWebPush(sub.endpoint);
        await sub.unsubscribe();
      }
      subscribed = false;
      return true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to unsubscribe';
      return false;
    } finally {
      loading = false;
    }
  }

  return {
    get supported() {
      return supported;
    },
    get subscribed() {
      return subscribed;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    refresh,
    subscribe,
    unsubscribe
  };
}
