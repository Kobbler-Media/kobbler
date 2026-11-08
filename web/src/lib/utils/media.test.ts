import { afterEach, describe, expect, it, vi } from 'vitest';
import { detectCapabilities } from './media.js';

const HEVC_MAIN10 = 'video/mp4; codecs="hvc1.2.4.L153.B0"';
const DOLBY_VISION = 'video/mp4; codecs="dvh1.05.06"';

function browser(opts: { decodes: string[]; hdrDisplay: boolean; firefox?: boolean }) {
  vi.stubGlobal('MediaSource', { isTypeSupported: (mime: string) => opts.decodes.includes(mime) });
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query === '(dynamic-range: high)' && opts.hdrDisplay
  }));
  vi.stubGlobal('navigator', {
    userAgent: opts.firefox
      ? 'Mozilla/5.0 Gecko/20100101 Firefox/140.0'
      : 'Mozilla/5.0 Chrome/140.0'
  });
}

describe('detectCapabilities', () => {
  afterEach(() => vi.unstubAllGlobals());

  it.each([
    [{ decodes: [], hdrDisplay: true }, []],
    [{ decodes: [HEVC_MAIN10], hdrDisplay: false }, ['hdr10', 'hlg']],
    [{ decodes: [HEVC_MAIN10], hdrDisplay: false, firefox: true }, []],
    [{ decodes: [HEVC_MAIN10], hdrDisplay: true, firefox: true }, ['hdr10', 'hlg']],
    [{ decodes: [HEVC_MAIN10, DOLBY_VISION], hdrDisplay: false }, ['hdr10', 'hlg']],
    [{ decodes: [HEVC_MAIN10, DOLBY_VISION], hdrDisplay: true }, ['hdr10', 'hlg', 'dolby_vision']]
  ])('reports the HDR a browser shows correctly: %o', (opts, hdr_formats) => {
    browser(opts);
    expect(detectCapabilities().hdr_formats).toEqual(hdr_formats);
  });
});
