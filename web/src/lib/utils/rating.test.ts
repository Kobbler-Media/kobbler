import { describe, expect, it } from 'vitest';
import {
  BOOK_RATING_SCALE,
  VIDEO_RATING_SCALE,
  formatRating,
  ratingScale,
  starFill
} from './rating';

describe('ratingScale', () => {
  it.each([
    ['book', BOOK_RATING_SCALE],
    ['movie', VIDEO_RATING_SCALE],
    ['tv', VIDEO_RATING_SCALE]
  ])('%s rates out of %i', (mediaType, scale) => {
    expect(ratingScale(mediaType)).toBe(scale);
  });
});

describe('formatRating', () => {
  it.each([
    [4.24, BOOK_RATING_SCALE, '4.2'],
    [7.33, VIDEO_RATING_SCALE, '7.3/10'],
    [8, VIDEO_RATING_SCALE, '8.0/10']
  ])('%d on a %i-point scale reads %s', (rating, scale, text) => {
    expect(formatRating(rating, scale)).toBe(text);
  });
});

describe('starFill', () => {
  it.each([
    [7, VIDEO_RATING_SCALE, [1, 1, 1, 0.5, 0]],
    [3.5, BOOK_RATING_SCALE, [1, 1, 1, 0.5, 0]],
    [10, VIDEO_RATING_SCALE, [1, 1, 1, 1, 1]],
    [null, VIDEO_RATING_SCALE, [0, 0, 0, 0, 0]]
  ])('%s out of %i fills five stars as %j', (rating, scale, fills) => {
    expect([1, 2, 3, 4, 5].map((star) => starFill(rating, scale, star))).toEqual(fills);
  });
});
