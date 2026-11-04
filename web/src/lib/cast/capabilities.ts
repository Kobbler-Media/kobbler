/**
 * Cast device codec capability profiles.
 *
 * Cast devices support different codecs depending on hardware generation.
 * These profiles tell Kobbler's playback decision engine what the Cast
 * device can handle, so it picks the right playback method.
 */

import type { ClientCapabilities } from '#lib/utils/media.js';

/**
 * Determine capabilities for a Cast device based on its model name.
 */
export function castCapabilities(deviceName: string): ClientCapabilities {
  const name = (deviceName || '').toLowerCase();

  const video_codecs = ['h264', 'vp9'];
  const audio_codecs = ['aac'];

  // Chromecast Ultra and Google TV devices support HEVC
  if (isGoogleTV(name) || isChromecastUltra(name)) {
    video_codecs.push('hevc');
  }

  // Google TV devices support Dolby Digital
  if (isGoogleTV(name)) {
    audio_codecs.push('ac3', 'eac3');
  }

  return { video_codecs, audio_codecs, containers: ['mp4'] };
}

function isGoogleTV(name: string): boolean {
  return (
    name.includes('google tv') ||
    name.includes('chromecast with google tv') ||
    name.includes('ccgtv') ||
    // Many third-party Google TV devices
    name.includes('shield') ||
    name.includes('onn') ||
    name.includes('bravia')
  );
}

function isChromecastUltra(name: string): boolean {
  return name.includes('ultra') || name.includes('chromecast ultra');
}

let reported: ClientCapabilities | null = null;
let waiters: Array<(caps: ClientCapabilities) => void> = [];

export function receiverReported(caps: ClientCapabilities | null): void {
  reported = caps;
  if (!caps) return;
  waiters.forEach((resolve) => resolve(caps));
  waiters = [];
}

/**
 * What the connected receiver probed itself for. It answers as a sender attaches,
 * which can be just after the handover begins.
 */
export function receiverCapabilities(
  deviceName: string,
  waitMs = 2000
): Promise<ClientCapabilities> {
  if (reported) return Promise.resolve(reported);
  return new Promise((resolve) => {
    const done = (caps: ClientCapabilities) => {
      clearTimeout(timer);
      resolve(caps);
    };
    const timer = setTimeout(() => {
      waiters = waiters.filter((w) => w !== done);
      resolve(castCapabilities(deviceName));
    }, waitMs);
    waiters.push(done);
  });
}
