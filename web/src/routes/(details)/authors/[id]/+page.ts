import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, fetch }) => {
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  try {
    const [authorResponse, booksResponse] = await Promise.all([
      fetch(`${apiBase}/api/v1/library/authors/${params.id}`, { credentials: 'include' }),
      fetch(`${apiBase}/api/v1/library/authors/${params.id}/books?include_not_owned=true`, {
        credentials: 'include'
      })
    ]);

    if (!authorResponse.ok) {
      return { author: null, books: [], series: [], error: 'Author not found' };
    }

    const author = await authorResponse.json();
    const books = booksResponse.ok ? await booksResponse.json() : [];

    // Get unique series from books
    const seriesIds = new Set<string>();
    for (const book of books) {
      if (book.series_id) {
        seriesIds.add(book.series_id);
      }
    }

    // Load series details
    const series = [];
    if (seriesIds.size > 0) {
      const seriesPromises = [...seriesIds].map((id) =>
        fetch(`${apiBase}/api/v1/library/series/${id}`, { credentials: 'include' })
          .then((r) => (r.ok ? r.json() : null))
          .catch(() => null)
      );
      const seriesResults = await Promise.all(seriesPromises);
      series.push(...seriesResults.filter((s) => s !== null));
    }

    return { author, books, series, error: null };
  } catch (e) {
    return {
      author: null,
      books: [],
      series: [],
      error: e instanceof Error ? e.message : 'Failed to load author'
    };
  }
};
