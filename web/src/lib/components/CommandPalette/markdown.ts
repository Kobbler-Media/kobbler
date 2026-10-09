import { marked } from 'marked';
import DOMPurify from 'isomorphic-dompurify';

// Configure marked for chat-style rendering
marked.use({ breaks: true, gfm: true });

/** Render a markdown string to sanitized HTML. */
export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text) as string);
}
