import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, fetch }) => {
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  try {
    const response = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${params.id}`, {
      credentials: 'include'
    });

    if (!response.ok) {
      return { item: null, error: 'Item not found' };
    }

    const item = await response.json();
    return { item, error: null };
  } catch (e) {
    return { item: null, error: e instanceof Error ? e.message : 'Failed to load item' };
  }
};
