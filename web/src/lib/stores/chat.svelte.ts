import { getChatStatus as fetchChatStatus, type ChatStatus } from '#lib/api/client.js';

export type { ChatStatus };

export interface ChatSource {
  type: string;
  id: string;
  title: string;
  url: string;
  coverUrl?: string;
  year?: number;
  description?: string;
}

export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string;
  sources?: ChatSource[];
}

let messages = $state<ChatMessage[]>([]);
let isStreaming = $state(false);
let error = $state<string | null>(null);
let isExpired = $state(false);
let chatStatus = $state<ChatStatus>({
  enabled: false,
  configured: false,
  provider: null,
  model: null
});
let statusLoaded = $state(false);
let abortController: AbortController | null = null;
let inactivityTimer: ReturnType<typeof setTimeout> | null = null;

const INACTIVITY_TIMEOUT_MS = 30_000;

function clearInactivityTimer() {
  if (inactivityTimer) {
    clearTimeout(inactivityTimer);
    inactivityTimer = null;
  }
}

function startInactivityTimer() {
  clearInactivityTimer();
  inactivityTimer = setTimeout(() => {
    // Mark conversation as expired — messages stay visible but next prompt starts fresh
    isExpired = true;
  }, INACTIVITY_TIMEOUT_MS);
}

export function getChatState() {
  return {
    get messages() {
      return messages;
    },
    get isStreaming() {
      return isStreaming;
    },
    get error() {
      return error;
    },
    get chatStatus() {
      return chatStatus;
    },
    get statusLoaded() {
      return statusLoaded;
    },
    get isExpired() {
      return isExpired;
    }
  };
}

export async function loadChatStatus() {
  try {
    chatStatus = await fetchChatStatus();
  } catch {
    // Chat not available, leave as disabled
  }
  statusLoaded = true;
}

export async function sendMessage(content: string) {
  if (!content.trim() || isStreaming) return;

  clearInactivityTimer();

  // Abort any previous in-flight stream
  if (abortController) {
    abortController.abort();
  }
  abortController = new AbortController();

  // If conversation expired, start fresh (don't send stale context to the LLM)
  if (isExpired) {
    messages = [];
    isExpired = false;
  }

  // Add user message
  messages = [...messages, { role: 'user', content: content.trim() }];
  isStreaming = true;
  error = null;

  // Build messages array for the API (full conversation history)
  const apiMessages = messages.map((m) => ({ role: m.role, content: m.content }));

  // Add placeholder assistant message for streaming
  messages = [...messages, { role: 'assistant', content: '' }];

  try {
    // The server resolves the model — reading it here would need the
    // admin-only system config.
    const response = await fetch('/api/v1/admin/chat/completions', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      signal: abortController.signal,
      body: JSON.stringify({
        messages: apiMessages,
        stream: true
      })
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(text || `Chat failed: ${response.status}`);
    }

    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    let currentEventType: string | null = null;

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });

      // Process SSE lines
      const lines = buffer.split('\n');
      buffer = lines.pop() || ''; // Keep incomplete line in buffer

      for (const line of lines) {
        // Track SSE event type
        if (line.startsWith('event: ')) {
          currentEventType = line.slice(7).trim();
          continue;
        }

        if (!line.startsWith('data: ')) continue;
        const data = line.slice(6).trim();
        if (data === '[DONE]') continue;

        // Handle sources event from backend
        if (currentEventType === 'sources') {
          try {
            const sources: ChatSource[] = JSON.parse(data).map((s: Record<string, unknown>) => ({
              type: s.type,
              id: s.id,
              title: s.title,
              url: s.url,
              coverUrl: s.cover_url as string | undefined,
              year: s.year as number | undefined,
              description: s.description as string | undefined
            }));
            // Attach sources to the last assistant message
            const lastIdx = messages.length - 1;
            messages = messages.map((m, i) => (i === lastIdx ? { ...m, sources } : m));
          } catch {
            // Skip malformed sources
          }
          currentEventType = null;
          continue;
        }

        // Handle regular content deltas
        currentEventType = null;
        try {
          const parsed = JSON.parse(data);
          const delta = parsed.choices?.[0]?.delta?.content;
          if (delta) {
            // Update the last (assistant) message
            const lastIdx = messages.length - 1;
            messages = messages.map((m, i) =>
              i === lastIdx ? { ...m, content: m.content + delta } : m
            );
          }
        } catch {
          // Skip non-JSON lines
        }
      }
    }
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') {
      // Aborted by clearChat — not an error
      return;
    }
    error = e instanceof Error ? e.message : 'Chat failed';
    // Remove the empty assistant message if we errored before getting content
    if (messages.length > 0 && messages[messages.length - 1].content === '') {
      messages = messages.slice(0, -1);
    }
  } finally {
    isStreaming = false;
    abortController = null;
    startInactivityTimer();
  }
}

/** Reset the inactivity timer — call when the user is actively typing. */
export function pokeInactivityTimer() {
  if (messages.length > 0 && !isStreaming) {
    startInactivityTimer();
  }
}

/** Pause the inactivity timer — call when input has text (user composing). */
export function pauseInactivityTimer() {
  clearInactivityTimer();
}

export function clearChat() {
  clearInactivityTimer();
  if (abortController) {
    abortController.abort();
    abortController = null;
  }
  messages = [];
  error = null;
  isStreaming = false;
  isExpired = false;
}
