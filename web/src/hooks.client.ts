import type { HandleClientError } from '@sveltejs/kit';

export const handleError: HandleClientError = ({ error, message }) => {
  console.error('[SvelteKit unhandled error]', message, error);
};

if (typeof window !== 'undefined') {
  // Browsers fire a window-level "error" with this message whenever a
  // ResizeObserver callback couldn't deliver all notifications in one frame.
  // It's benign (the next frame catches up) and gets emitted continuously by
  // some libraries (e.g. foliate-js's paginator), so swallow it here instead
  // of drowning the console.
  const isResizeObserverLoop = (msg: string | undefined) =>
    typeof msg === 'string' && msg.includes('ResizeObserver loop');

  window.addEventListener('error', (event) => {
    if (isResizeObserverLoop(event.message)) {
      event.stopImmediatePropagation();
      return;
    }
    console.error('[Uncaught error]', event.message, event.error);
  });

  window.addEventListener('unhandledrejection', (event) => {
    console.error('[Unhandled promise rejection]', event.reason);
  });
}
