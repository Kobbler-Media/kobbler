<script lang="ts">
  import { page } from '$app/stores';
  import DiscoverDetailPage from '$lib/components/discover/DiscoverDetailPage.svelte';
  import { consumeDiscoverPreview } from '$lib/stores/discover-preview.svelte';

  const mediaType = $derived($page.params.media_type ?? '');
  const provider = $derived($page.params.provider ?? '');
  const providerId = $derived($page.params.provider_id ?? '');
  const focusSeason = $derived(Number($page.url.searchParams.get('season')) || null);
</script>

{#key `${mediaType}/${provider}/${providerId}`}
  <DiscoverDetailPage
    {mediaType}
    {provider}
    {providerId}
    {focusSeason}
    preview={consumeDiscoverPreview(mediaType, provider, providerId)}
  />
{/key}
