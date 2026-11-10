<script lang="ts">
  import type { Book } from '#lib/types.js';
  import * as api from '#lib/api/client.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';

  interface Props {
    book: Book;
    context: 'library' | 'ingest';
  }

  let { book, context }: Props = $props();

  const auth = getAuthState();

  let status = $state<Awaited<ReturnType<typeof api.hasTranscript>> | null>(null);
  let transcribing = $state(false);
  let extracting = $state(false);

  let hasEbook = $derived((book.formats?.length ?? 0) > 0);
  let extracted = $derived(status?.has_ebook_segments ?? false);
  let showTranscript = $derived(book.has_audiobook && !!status && status.transcript_mode !== 'off');
  let showCast = $derived(
    context === 'library' && auth.isAdmin && hasEbook && !!status?.full_cast_available
  );
  let visible = $derived(!!status && (hasEbook || showTranscript));

  $effect(() => {
    const id = book.id;
    if (!(book.has_audiobook || hasEbook)) return;
    api
      .hasTranscript(id)
      .then((s) => (status = s))
      .catch(() => (status = null));
  });

  async function extract() {
    extracting = true;
    try {
      await api.extractEbookText(book.id);
    } catch {
      extracting = false;
    }
  }

  async function transcribe() {
    transcribing = true;
    try {
      await api.transcribeAudiobook(book.id);
    } catch {
      transcribing = false;
    }
  }
</script>

{#if visible}
  <div class="section card-section">
    <div class="label">Text &amp; narration</div>
    <ul class="rows">
      {#if hasEbook}
        <li>
          <span class="material-icons-outlined icon">text_snippet</span>
          <div class="what">
            <span class="name">Ebook text</span>
            <span class="why">
              {extracted
                ? 'Extracted — read-along and full cast can use it'
                : 'Needed for read-along and a full-cast audiobook'}
            </span>
          </div>
          {#if extracted}
            <span class="material-icons-outlined ok" title="Extracted">check_circle</span>
          {:else}
            <button type="button" class="btn alt small" disabled={extracting} onclick={extract}>
              {extracting ? 'Started' : 'Extract'}
            </button>
          {/if}
        </li>
      {/if}
      {#if showTranscript}
        <li>
          <span class="material-icons-outlined icon">mic</span>
          <div class="what">
            <span class="name">Audiobook transcript</span>
            <span class="why">
              {status?.has_audio_transcript
                ? 'Transcribed — the audiobook follows along with the text'
                : 'Lets the audiobook follow along with the text'}
            </span>
          </div>
          {#if status?.has_audio_transcript}
            <span class="material-icons-outlined ok" title="Transcribed">check_circle</span>
          {:else}
            <button
              type="button"
              class="btn alt small"
              disabled={transcribing}
              onclick={transcribe}
            >
              {transcribing ? 'Started' : 'Transcribe'}
            </button>
          {/if}
        </li>
      {/if}
      {#if showCast}
        <li>
          <span class="material-icons-outlined icon">record_voice_over</span>
          <div class="what">
            <span class="name">Full-cast audiobook</span>
            <span class="why">
              {extracted
                ? 'A voice for every character, made from the ebook'
                : 'Extract the ebook text first'}
            </span>
          </div>
          {#if extracted}
            <a class="btn alt small" href="/library/books/{book.id}/cast">Open</a>
          {/if}
        </li>
      {/if}
    </ul>
  </div>
{/if}

<style>
  .rows {
    display: grid;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 0;
  }

  li + li {
    border-top: 1px solid var(--ring);
  }

  .icon {
    font-size: 20px;
    color: var(--muted);
  }

  .what {
    display: grid;
    flex: 1;
    min-width: 0;
  }

  .name {
    font-size: 14px;
    font-weight: 500;
  }

  .why {
    font-size: 12px;
    color: var(--muted);
  }

  .ok {
    font-size: 20px;
    color: var(--success);
  }

  a.btn {
    text-decoration: none;
  }
</style>
