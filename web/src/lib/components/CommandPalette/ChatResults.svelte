<script lang="ts">
  import { goto } from '$app/navigation';
  import { getChatState, type ChatSource } from '$lib/stores/chat.svelte';
  import { closeCommandPalette } from '$lib/stores/ui.svelte';
  import { renderMarkdown } from './markdown';

  const chat = getChatState();
  let scrollContainer: HTMLDivElement | undefined = $state();

  // Auto-scroll to bottom when messages change
  $effect(() => {
    const _ = chat.messages.length;
    // Also react to content changes on the last message
    const last = chat.messages.at(-1);
    const __ = last?.content.length;
    if (scrollContainer) {
      requestAnimationFrame(() => {
        if (scrollContainer) {
          scrollContainer.scrollTop = scrollContainer.scrollHeight;
        }
      });
    }
  });

  function navigateToSource(source: ChatSource) {
    goto(source.url);
    closeCommandPalette();
  }

  function placeholderIcon(type: string): string {
    switch (type) {
      case 'movie':
        return 'movie';
      case 'tv_show':
      case 'tv_season':
      case 'tv_episode':
        return 'tv';
      case 'book':
        return 'auto_stories';
      case 'author':
        return 'person';
      case 'series':
        return 'collections_bookmark';
      default:
        return 'search';
    }
  }
</script>

<div class="chat-results" bind:this={scrollContainer}>
  {#if chat.messages.length === 0 && !chat.error}
    <div class="chat-empty">
      <p>Ask anything about your library</p>
      <div class="chat-suggestions">
        <span>Who's that actor from...</span>
        <span>What audiobook had that scene where...</span>
        <span>Show me unfinished TV shows</span>
        <span>Recommend something like...</span>
      </div>
    </div>
  {:else}
    {#each chat.messages as message, i (i)}
      <div
        class="chat-message"
        class:user={message.role === 'user'}
        class:assistant={message.role === 'assistant'}
      >
        {#if message.role === 'user'}
          <div class="message-bubble user-bubble">
            <span class="message-text">{message.content}</span>
          </div>
        {:else}
          <div class="assistant-block">
            {#if message.content === '' && chat.isStreaming}
              <span class="typing-indicator">
                <span></span><span></span><span></span>
              </span>
            {:else}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- markdown output sanitized with DOMPurify -->
              <div class="message-markdown">{@html renderMarkdown(message.content)}</div>
            {/if}

            {#if message.sources && message.sources.length > 0}
              <div class="source-strip">
                {#each message.sources as source, si (source.id)}
                  <button
                    class="source-tile"
                    type="button"
                    onclick={() => navigateToSource(source)}
                    style:animation-delay="{si * 50}ms"
                  >
                    <div class="source-poster">
                      {#if source.coverUrl}
                        <img
                          src={source.coverUrl}
                          alt=""
                          class="source-poster-img"
                          loading="lazy"
                          onerror={(e) => {
                            (e.currentTarget as HTMLImageElement).style.display = 'none';
                          }}
                        />
                      {/if}
                      <span class="material-icons-outlined source-placeholder-icon">
                        {placeholderIcon(source.type)}
                      </span>
                    </div>
                    <div class="source-meta">
                      <span class="source-title">{source.title}</span>
                      <span class="source-sub">
                        {#if source.year}{source.year}{:else if source.description}{source.description}{/if}
                      </span>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/each}
  {/if}

  {#if chat.isExpired}
    <div class="chat-expired">Session expired — your next message starts a new conversation</div>
  {/if}

  {#if chat.error}
    <div class="chat-error">{chat.error}</div>
  {/if}
</div>

<style>
  .chat-results {
    max-height: 400px;
    overflow-y: auto;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    scroll-behavior: smooth;
  }

  .chat-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 24px 0;
    color: var(--muted);
    text-align: center;
  }

  .chat-empty p {
    font-size: 14px;
    margin: 0;
  }

  .chat-suggestions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: center;
  }

  .chat-suggestions span {
    font-size: 11px;
    padding: 4px 10px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 12px;
    color: var(--muted);
  }

  :global([data-theme='light']) .chat-suggestions span {
    background: rgba(0, 0, 0, 0.05);
  }

  .chat-message {
    display: flex;
  }

  .chat-message.user {
    justify-content: flex-end;
  }

  .chat-message.assistant {
    justify-content: flex-start;
  }

  /* --- User bubble (unchanged) --- */
  .user-bubble {
    max-width: 85%;
    padding: 8px 12px;
    border-radius: 12px;
    font-size: 13px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    background: var(--accent);
    color: white;
    border-bottom-right-radius: 4px;
  }

  .message-text {
    display: block;
  }

  /* --- Assistant content block --- */
  .assistant-block {
    width: 100%;
    padding: 12px 16px;
    border-radius: 8px;
    border-left: 3px solid var(--accent);
    background: rgba(255, 255, 255, 0.03);
  }

  :global([data-theme='light']) .assistant-block {
    background: rgba(0, 0, 0, 0.02);
  }

  /* --- Markdown typography --- */
  .message-markdown {
    font-size: 13px;
    line-height: 1.6;
    color: var(--ink);
    word-break: break-word;
  }

  .message-markdown :global(p) {
    margin: 0 0 0.5em;
  }

  .message-markdown :global(p:last-child) {
    margin-bottom: 0;
  }

  .message-markdown :global(strong) {
    font-weight: 600;
  }

  .message-markdown :global(em) {
    font-style: italic;
  }

  .message-markdown :global(ul),
  .message-markdown :global(ol) {
    margin: 0.4em 0;
    padding-left: 1.4em;
  }

  .message-markdown :global(li) {
    margin: 0.2em 0;
  }

  .message-markdown :global(code) {
    background: var(--tag-bg);
    padding: 1px 5px;
    border-radius: 4px;
    font-size: 12px;
    font-family: monospace;
  }

  .message-markdown :global(pre) {
    background: var(--tag-bg);
    padding: 8px 10px;
    border-radius: 6px;
    overflow-x: auto;
    margin: 0.5em 0;
  }

  .message-markdown :global(pre code) {
    background: none;
    padding: 0;
  }

  .message-markdown :global(a) {
    color: var(--accent);
    text-decoration: none;
  }

  .message-markdown :global(a:hover) {
    text-decoration: underline;
  }

  .message-markdown :global(h1),
  .message-markdown :global(h2),
  .message-markdown :global(h3),
  .message-markdown :global(h4) {
    font-weight: 600;
    margin: 0.6em 0 0.3em;
    line-height: 1.3;
  }

  .message-markdown :global(h1) {
    font-size: 16px;
  }

  .message-markdown :global(h2) {
    font-size: 15px;
  }

  .message-markdown :global(h3),
  .message-markdown :global(h4) {
    font-size: 14px;
  }

  .message-markdown :global(blockquote) {
    border-left: 2px solid var(--ring);
    padding-left: 10px;
    color: var(--muted);
    margin: 0.5em 0;
  }

  .message-markdown :global(hr) {
    border: none;
    border-top: 1px solid var(--ring);
    margin: 0.8em 0;
  }

  /* --- Typing indicator --- */
  .typing-indicator {
    display: flex;
    gap: 4px;
    padding: 4px 0;
  }

  .typing-indicator span {
    width: 6px;
    height: 6px;
    background: var(--muted);
    border-radius: 50%;
    animation: typing 1.2s ease-in-out infinite;
  }

  .typing-indicator span:nth-child(2) {
    animation-delay: 0.2s;
  }

  .typing-indicator span:nth-child(3) {
    animation-delay: 0.4s;
  }

  @keyframes typing {
    0%,
    60%,
    100% {
      transform: translateY(0);
    }
    30% {
      transform: translateY(-4px);
    }
  }

  /* --- Source tile strip --- */
  .source-strip {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding: 10px 0 2px;
    margin-top: 8px;
    border-top: 1px solid var(--ring);
    scrollbar-width: none;
  }

  .source-strip::-webkit-scrollbar {
    display: none;
  }

  .source-tile {
    flex-shrink: 0;
    width: 100px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
    cursor: pointer;
    padding: 0;
    text-align: left;
    color: inherit;
    font: inherit;
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease;
    animation: tile-appear 0.3s ease both;
  }

  .source-tile:hover {
    transform: translateY(-2px);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.3);
  }

  :global([data-theme='light']) .source-tile:hover {
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.1);
  }

  @keyframes tile-appear {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .source-poster {
    width: 100px;
    height: 150px;
    position: relative;
    overflow: hidden;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  :global([data-theme='light']) .source-poster {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
  }

  .source-poster-img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    z-index: 1;
  }

  .source-placeholder-icon {
    font-size: 32px;
    color: var(--muted);
    opacity: 0.3;
    z-index: 0;
  }

  .source-meta {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 6px 8px 8px;
    min-width: 0;
  }

  .source-title {
    font-size: 12px;
    font-weight: 600;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--ink);
  }

  .source-sub {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* --- Expired indicator --- */
  .chat-expired {
    padding: 6px 12px;
    border-radius: 8px;
    font-size: 11px;
    text-align: center;
    background: rgba(255, 255, 255, 0.04);
    color: var(--muted);
    border: 1px dashed var(--ring);
  }

  :global([data-theme='light']) .chat-expired {
    background: rgba(0, 0, 0, 0.02);
  }

  /* --- Error --- */
  .chat-error {
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 12px;
    background: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
  }
</style>
