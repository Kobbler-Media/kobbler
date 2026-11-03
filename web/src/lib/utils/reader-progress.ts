import { saveReaderProgress } from '#lib/api/client.js';

const DEBOUNCE_MS = 3000;
const PERIODIC_MS = 60000;
const TRACKING_KEY_PREFIX = 'kobbler-reader-track-';

/** Check if tracking is enabled for a book (persisted in localStorage). */
export function isTrackingEnabled(bookId: string): boolean {
  try {
    return localStorage.getItem(`${TRACKING_KEY_PREFIX}${bookId}`) === 'true';
  } catch {
    return false;
  }
}

/** Set tracking enabled/disabled for a book. */
export function setTrackingEnabled(bookId: string, enabled: boolean): void {
  try {
    if (enabled) {
      localStorage.setItem(`${TRACKING_KEY_PREFIX}${bookId}`, 'true');
    } else {
      localStorage.removeItem(`${TRACKING_KEY_PREFIX}${bookId}`);
    }
  } catch {
    // localStorage unavailable
  }
}

/** Manages debounced + periodic progress saving for the web reader. */
export class ProgressTracker {
  private bookId: string;
  private lastProgress = 0;
  private lastCfi: string | undefined;
  private debounceTimer: ReturnType<typeof setTimeout> | null = null;
  private periodicTimer: ReturnType<typeof setInterval> | null = null;
  private lastSaveTime = Date.now();
  private dirty = false;

  constructor(bookId: string) {
    this.bookId = bookId;
  }

  /** Start periodic saves. */
  start(): void {
    this.lastSaveTime = Date.now();
    this.periodicTimer = setInterval(() => this.flush(), PERIODIC_MS);
  }

  /** Update progress (debounced save). */
  update(progressPercent: number, cfi?: string): void {
    this.lastProgress = progressPercent;
    this.lastCfi = cfi;
    this.dirty = true;

    if (this.debounceTimer) clearTimeout(this.debounceTimer);
    this.debounceTimer = setTimeout(() => this.flush(), DEBOUNCE_MS);
  }

  /** Immediately flush pending progress to server. */
  async flush(closing = false): Promise<void> {
    if (!this.dirty && !closing) return;
    this.dirty = false;

    const now = Date.now();
    const minutesSinceLastSave = Math.round((now - this.lastSaveTime) / 60000);
    this.lastSaveTime = now;

    try {
      await saveReaderProgress(this.bookId, {
        progress_percent: this.lastProgress,
        cfi: this.lastCfi,
        minutes_since_last_save: minutesSinceLastSave > 0 ? minutesSinceLastSave : undefined,
        closing
      });
    } catch (e) {
      // Re-mark dirty so next flush retries
      this.dirty = true;
      console.warn('Failed to save reading progress:', e);
    }
  }

  /** Stop tracking and flush with closing flag. */
  async destroy(): Promise<void> {
    if (this.debounceTimer) clearTimeout(this.debounceTimer);
    if (this.periodicTimer) clearInterval(this.periodicTimer);
    await this.flush(true);
  }
}
