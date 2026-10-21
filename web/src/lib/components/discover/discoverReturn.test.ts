import { describe, expect, it, vi } from 'vitest';
import { createDiscoverReturn } from './discoverReturn';

function setup(playing = false) {
  const win = new EventTarget();
  const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' });
  const navigate = vi.fn();
  const recheck = vi.fn();
  const state = { playing };
  const ret = createDiscoverReturn({
    mediaType: 'movie',
    navigate,
    recheck,
    isPlaying: () => state.playing
  });
  const stop = ret.watch(win, doc);
  const pageshow = (persisted: boolean) =>
    win.dispatchEvent(Object.assign(new Event('pageshow'), { persisted }));
  const visible = (visibilityState: string) => {
    doc.visibilityState = visibilityState;
    doc.dispatchEvent(new Event('visibilitychange'));
  };
  return { ret, navigate, recheck, state, stop, pageshow, visible };
}

describe('discover return', () => {
  it('replaces the discover entry with the library page, once', () => {
    const { ret, navigate } = setup();
    expect(ret.redirect(null)).toBe(false);
    expect(ret.redirect('m1')).toBe(true);
    expect(ret.redirect('m1')).toBe(true);
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenCalledWith('/movies/m1', { replaceState: true });
  });

  it('looks again when the page is shown from the back/forward cache', () => {
    const { pageshow, recheck } = setup();
    pageshow(false);
    expect(recheck).not.toHaveBeenCalled();
    pageshow(true);
    expect(recheck).toHaveBeenCalledTimes(1);
  });

  it('looks again when the tab comes back into view', () => {
    const { visible, recheck } = setup();
    visible('hidden');
    expect(recheck).not.toHaveBeenCalled();
    visible('visible');
    expect(recheck).toHaveBeenCalledTimes(1);
  });

  it('never pulls the page out from under a playing stream', () => {
    const { visible, pageshow, recheck, ret, navigate } = setup(true);
    visible('visible');
    pageshow(true);
    expect(recheck).not.toHaveBeenCalled();
    expect(ret.redirect('m1')).toBe(false);
    expect(navigate).not.toHaveBeenCalled();
  });

  it('looks again after a stream that played, since a cached file may not name the item', () => {
    const { ret, recheck } = setup();
    ret.streamEnded(false);
    expect(recheck).not.toHaveBeenCalled();
    ret.streamEnded(true);
    expect(recheck).toHaveBeenCalledTimes(1);
  });

  it('stops listening when the page goes', () => {
    const { stop, visible, recheck } = setup();
    stop();
    visible('visible');
    expect(recheck).not.toHaveBeenCalled();
  });
});
