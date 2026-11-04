import { describe, expect, it } from 'vitest';
import { getAudioStreamUrl } from './client';

describe('getAudioStreamUrl', () => {
  it('asks for the mp4 container so an mp3 seeks to the time the element reports', () => {
    expect(getAudioStreamUrl('b1', 'f1')).toBe('/api/v1/admin/books/b1/audio/f1?container=mp4');
  });
});
