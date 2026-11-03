import { discoverLibraryRoute } from '#lib/utils/entity-routes.js';

interface Deps {
  mediaType: string;
  navigate: (route: string, opts: { replace: true }) => void;
  recheck: () => void;
  isPlaying: () => boolean;
}

type Visible = EventTarget & { visibilityState: string };

// A stream mints the library item, so a discover page can go stale under the
// viewer; it replaces itself with the library page so Back cannot loop into it.
export function createDiscoverReturn(deps: Deps) {
  let redirected = false;

  function redirect(libraryId: string | null | undefined): boolean {
    if (redirected) return true;
    const route = discoverLibraryRoute(deps.mediaType, libraryId);
    if (!route || deps.isPlaying()) return false;
    redirected = true;
    deps.navigate(route, { replace: true });
    return true;
  }

  function recheck() {
    if (!redirected && !deps.isPlaying()) deps.recheck();
  }

  function watch(win: EventTarget, doc: Visible): () => void {
    const onShow = (e: Event) => {
      if ((e as PageTransitionEvent).persisted) recheck();
    };
    const onVisibility = () => {
      if (doc.visibilityState === 'visible') recheck();
    };
    win.addEventListener('pageshow', onShow);
    doc.addEventListener('visibilitychange', onVisibility);
    return () => {
      win.removeEventListener('pageshow', onShow);
      doc.removeEventListener('visibilitychange', onVisibility);
    };
  }

  function streamEnded(played: boolean) {
    if (played) recheck();
  }

  return { redirect, watch, streamEnded };
}
