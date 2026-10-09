import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, fetch }) => {
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  try {
    const response = await fetch(`${apiBase}/api/v1/library/books/${params.id}`, {
      credentials: 'include'
    });

    if (!response.ok) {
      return { book: null, error: 'Book not found' };
    }

    const book = await response.json();
    return { book, error: null };
  } catch (e) {
    return { book: null, error: e instanceof Error ? e.message : 'Failed to load book' };
  }
};
