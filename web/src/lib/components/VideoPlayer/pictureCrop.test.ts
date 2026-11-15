import { describe, expect, it } from 'vitest';
import {
  cropArea,
  cuePlacement,
  frameStyle,
  placeCues,
  resolveFit,
  sessionPicture,
  videoFrameRect,
  type FrameRect,
  type PictureArea
} from './pictureCrop';

const whole: PictureArea = { x: 0, y: 0, width: 1, height: 1 };
const letterbox: PictureArea = { x: 0, y: 0.125, width: 1, height: 0.75 };
const pillarbox: PictureArea = { x: 0.125, y: 0, width: 0.75, height: 1 };
const offCentre: PictureArea = { x: 0, y: 0.1, width: 1, height: 0.75 };

function expectRect(actual: FrameRect, expected: FrameRect) {
  for (const key of ['left', 'top', 'width', 'height'] as const) {
    expect(actual[key], key).toBeCloseTo(expected[key], 6);
  }
}

describe('cropArea', () => {
  it.each<[string, PictureArea | null | undefined, string | null | undefined, PictureArea | null]>([
    ['no measurement', undefined, null, null],
    ['null measurement', null, null, null],
    ['whole frame', whole, null, null],
    ['whole frame within rounding', { x: 0, y: 0.0004, width: 1, height: 0.9995 }, null, null],
    ['zero width', { x: 0, y: 0, width: 0, height: 1 }, null, null],
    ['negative height', { x: 0, y: 0, width: 1, height: -0.5 }, null, null],
    ['overflowing the frame', { x: 0.5, y: 0, width: 0.75, height: 1 }, null, null],
    ['not a number', { x: 0, y: Number.NaN, width: 1, height: 0.5 }, null, null],
    ['letterbox', letterbox, null, letterbox],
    ['letterbox with text subtitles', letterbox, 'vtt', letterbox],
    ['letterbox with frame-anchored ass', letterbox, 'ass', null],
    ['letterbox with burned-in subtitles', letterbox, 'burn_in', null]
  ])('%s', (_name, picture, subtitleFormat, expected) => {
    expect(cropArea(picture, subtitleFormat)).toEqual(expected);
  });
});

describe('sessionPicture', () => {
  const fromResponse = { session_id: 'a', picture: letterbox };
  it.each<
    [
      string,
      { session_id: string; picture?: PictureArea },
      { sessionId: string; picture: PictureArea } | null,
      PictureArea | undefined
    ]
  >([
    ['response only', fromResponse, null, letterbox],
    ['nothing known', { session_id: 'a' }, null, undefined],
    [
      'late measurement for this session',
      { session_id: 'a' },
      { sessionId: 'a', picture: pillarbox },
      pillarbox
    ],
    [
      'late measurement outranks the response',
      fromResponse,
      { sessionId: 'a', picture: pillarbox },
      pillarbox
    ],
    [
      'measurement of the previous title',
      { session_id: 'b' },
      { sessionId: 'a', picture: pillarbox },
      undefined
    ],
    [
      'previous title never outranks the response',
      { session_id: 'b', picture: letterbox },
      { sessionId: 'a', picture: pillarbox },
      letterbox
    ]
  ])('%s', (_name, session, late, expected) => {
    expect(sessionPicture(session, late)).toEqual(expected);
  });
});

describe('resolveFit', () => {
  it.each<[string, 'auto' | 'fit' | 'fill', number, number, number, number, 'contain' | 'cover']>([
    ['fit', 'fit', 1920, 1080, 1920, 1080, 'contain'],
    ['fill', 'fill', 1920, 800, 1920, 1080, 'cover'],
    ['auto, same shape', 'auto', 1920, 1080, 1280, 720, 'cover'],
    ['auto, within tolerance', 'auto', 1920, 1080, 1920, 1200, 'cover'],
    ['auto, scope on 16:9', 'auto', 1920, 800, 1920, 1080, 'contain'],
    ['auto, unknown content', 'auto', 0, 0, 1920, 1080, 'contain'],
    ['auto, unknown container', 'auto', 1920, 1080, 0, 0, 'contain']
  ])('%s', (_name, mode, w, h, cw, ch, expected) => {
    expect(resolveFit(mode, w, h, cw, ch)).toBe(expected);
  });
});

describe('videoFrameRect', () => {
  it.each<[string, 'contain' | 'cover', number, number, number, number, PictureArea, FrameRect]>([
    [
      'scope picture in a 16:9 encode fills a scope screen',
      'contain',
      1920,
      1080,
      2560,
      1080,
      letterbox,
      { left: 0, top: -180, width: 2560, height: 1440 }
    ],
    [
      'scope picture on a 16:9 screen keeps only the screen-shape bars',
      'contain',
      1920,
      1080,
      1920,
      1080,
      letterbox,
      { left: 0, top: 0, width: 1920, height: 1080 }
    ],
    [
      'pillarboxed 4:3 picture on a 16:9 screen',
      'contain',
      1920,
      1080,
      1920,
      1080,
      pillarbox,
      { left: 0, top: 0, width: 1920, height: 1080 }
    ],
    [
      'pillarboxed picture on a matching screen',
      'contain',
      1920,
      1080,
      1440,
      1080,
      pillarbox,
      { left: -240, top: 0, width: 1920, height: 1080 }
    ],
    [
      'off-centre picture is centred',
      'contain',
      1920,
      1080,
      2560,
      1080,
      offCentre,
      { left: 0, top: -144, width: 2560, height: 1440 }
    ],
    [
      'cover fills the screen with the picture',
      'cover',
      1920,
      1080,
      1920,
      1080,
      letterbox,
      { left: -320, top: -180, width: 2560, height: 1440 }
    ],
    [
      'scaled-down source',
      'contain',
      1280,
      720,
      2560,
      1080,
      letterbox,
      { left: 0, top: -180, width: 2560, height: 1440 }
    ]
  ])('%s', (_name, fit, vw, vh, cw, ch, area, expected) => {
    expectRect(videoFrameRect(fit, vw, vh, cw, ch, area)!, expected);
  });

  it.each<[string, number, number, number, number]>([
    ['unknown video size', 0, 0, 1920, 1080],
    ['unlaid-out container', 1920, 1080, 0, 0]
  ])('has no rect for %s', (_name, vw, vh, cw, ch) => {
    expect(videoFrameRect('contain', vw, vh, cw, ch, letterbox)).toBeNull();
  });
});

describe('frameStyle', () => {
  it('pins the whole frame to the rect', () => {
    expect(frameStyle({ left: -240, top: 0, width: 1920, height: 1080.5 })).toBe(
      'left: -240px; top: 0px; width: 1920px; height: 1080.5px;'
    );
  });
});

describe('cuePlacement', () => {
  it.each<[string, FrameRect, number, number, { line: number; position: number; size: number }]>([
    [
      'bars hidden above and below',
      { left: 0, top: -180, width: 2560, height: 1440 },
      2560,
      1080,
      { line: 85.25, position: 50, size: 100 }
    ],
    [
      'bars hidden left and right',
      { left: -240, top: 0, width: 1920, height: 1080 },
      1440,
      1080,
      { line: 97, position: 50, size: 75 }
    ],
    [
      'off-centre frame',
      { left: -400, top: 0, width: 2000, height: 1000 },
      1000,
      1000,
      { line: 97, position: 45, size: 50 }
    ],
    [
      'frame smaller than the screen',
      { left: 0, top: 140, width: 1920, height: 800 },
      1920,
      1080,
      { line: 97, position: 50, size: 100 }
    ]
  ])('%s', (_name, rect, cw, ch, expected) => {
    const placement = cuePlacement(rect, cw, ch);
    expect(placement.line).toBeCloseTo(expected.line, 6);
    expect(placement.position).toBeCloseTo(expected.position, 6);
    expect(placement.size).toBeCloseTo(expected.size, 6);
  });
});

describe('placeCues', () => {
  const cue = () => ({
    snapToLines: true,
    line: 'auto',
    lineAlign: 'start',
    position: 'auto',
    size: 100
  });

  it('moves cues into the visible picture and back', () => {
    const cues = [cue(), cue()];
    const track = { cues } as unknown as TextTrack;

    placeCues(track, { line: 85.25, position: 45, size: 50 });
    for (const c of cues) {
      expect(c).toEqual({
        snapToLines: false,
        line: 85.25,
        lineAlign: 'end',
        position: 45,
        size: 50
      });
    }

    placeCues(track, null);
    for (const c of cues) expect(c).toEqual(cue());
  });

  it('tolerates a track without cues', () => {
    expect(() => placeCues({ cues: null } as unknown as TextTrack, null)).not.toThrow();
  });
});
