import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, fetch }) => {
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  try {
    const [bookRes, audioRes] = await Promise.all([
      fetch(`${apiBase}/api/v1/library/books/${params.id}`, { credentials: 'include' }),
      fetch(`${apiBase}/api/v1/admin/books/${params.id}/audio-files`, { credentials: 'include' })
    ]);

    if (!bookRes.ok) {
      return {
        book: null,
        audioFileIds: [],
        audioFileDurations: [],
        audioFileIntroOutro: {},
        error: 'Book not found'
      };
    }

    const book = await bookRes.json();
    let audioFileIds: string[] = [];
    let audioFileDurations: number[] = [];
    let audioFileIntroOutro: Record<
      string,
      { intro_end_seconds?: number | null; outro_start_seconds?: number | null }
    > = {};

    if (audioRes.ok) {
      const audioFiles: {
        id: string;
        available: boolean;
        duration_seconds?: number | null;
        intro_end_seconds?: number | null;
        outro_start_seconds?: number | null;
      }[] = await audioRes.json();
      const available = audioFiles.filter((f) => f.available);
      audioFileIds = available.map((f) => f.id);
      audioFileDurations = available.map((f) => f.duration_seconds ?? 0);
      for (const f of available) {
        if (f.intro_end_seconds != null || f.outro_start_seconds != null) {
          audioFileIntroOutro[f.id] = {
            intro_end_seconds: f.intro_end_seconds,
            outro_start_seconds: f.outro_start_seconds
          };
        }
      }
    }

    return { book, audioFileIds, audioFileDurations, audioFileIntroOutro, error: null };
  } catch (e) {
    return {
      book: null,
      audioFileIds: [],
      audioFileDurations: [],
      audioFileIntroOutro: {},
      error: e instanceof Error ? e.message : 'Failed to load book'
    };
  }
};
