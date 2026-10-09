import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, fetch }) => {
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  try {
    const [seriesResponse, booksResponse] = await Promise.all([
      fetch(`${apiBase}/api/v1/library/series/${params.id}`, { credentials: 'include' }),
      fetch(`${apiBase}/api/v1/library/series/${params.id}/books?include_not_owned=true`, {
        credentials: 'include'
      })
    ]);

    if (!seriesResponse.ok) {
      return { series: null, books: [], error: 'Series not found' };
    }

    const series = await seriesResponse.json();
    const books = booksResponse.ok ? await booksResponse.json() : [];

    return { series, books, error: null };
  } catch (e) {
    return {
      series: null,
      books: [],
      error: e instanceof Error ? e.message : 'Failed to load series'
    };
  }
};
