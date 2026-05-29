/**
 * Tests for parser.format() — specifically the oversize-line warning
 * added after a freeq-society rebuttal-prompt with an 8.6KB body was
 * silently truncated to ~8KB on the wire, leaving the receiving
 * panelist with broken JSON it couldn't parse. The warning surfaces
 * the size mismatch BEFORE the caller commits the message to the wire,
 * so debugging starts from "stderr told me the line was 8.6KB" instead
 * of "JSON.parse failed and the moderator hung silently."
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  format,
  LINE_SIZE_WARN_THRESHOLD,
  TAG_SIZE_WARN_THRESHOLD,
} from './parser';

describe('format() oversize warning', () => {
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    warnSpy.mockRestore();
  });

  it('does not warn for normal-sized messages', () => {
    format('PRIVMSG', ['#channel', 'hello world'], {
      msgid: '01ABC',
      'freeq.at/event': 'commit',
    });
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it('does not warn at exactly the line threshold', () => {
    // Body sized so the full line lands at exactly LINE_SIZE_WARN_THRESHOLD.
    // "PRIVMSG #c :<body>" → command(7) + space(1) + target(2) + space(2) + colon(1) + body
    const overhead = 'PRIVMSG #c :'.length;
    const body = 'x'.repeat(LINE_SIZE_WARN_THRESHOLD - overhead);
    format('PRIVMSG', ['#c', body]);
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it('warns when the full line exceeds the threshold', () => {
    // Body contains a space so format() adds the `: ` prefix (2 extra
    // chars on top of the leading ` `). Without that, the body would be
    // attached as a plain trailing param and the line shape differs.
    const body = 'a ' + 'x'.repeat(LINE_SIZE_WARN_THRESHOLD + 100);
    format('PRIVMSG', ['#cloudcity', body]);

    expect(warnSpy).toHaveBeenCalledTimes(1);
    const msg = warnSpy.mock.calls[0][0] as string;
    expect(msg).toContain('oversize PRIVMSG to #cloudcity');
    expect(msg).toMatch(/line=\d+B \(warn>7000\)/);
    expect(msg).toContain('Server may truncate or drop silently');
  });

  it('warns when tags alone exceed the tag threshold (even if line is fine)', () => {
    // One short tag whose value is bigger than the per-spec tag limit.
    // PRIVMSG body stays small but the tag portion blows the budget.
    const bigValue = 'a'.repeat(TAG_SIZE_WARN_THRESHOLD + 50);
    format('PRIVMSG', ['#c', 'short body'], {
      'freeq.at/payload': bigValue,
    });

    expect(warnSpy).toHaveBeenCalledTimes(1);
    const msg = warnSpy.mock.calls[0][0] as string;
    expect(msg).toMatch(/tags=\d+B/);
  });

  it('emits at most one warning per format() call', () => {
    // Both thresholds breached by the same call — still just one warning.
    const bigTag = 'a'.repeat(TAG_SIZE_WARN_THRESHOLD + 100);
    const bigBody = 'x'.repeat(LINE_SIZE_WARN_THRESHOLD + 100);
    format('PRIVMSG', ['#c', bigBody], { 'freeq.at/payload': bigTag });
    expect(warnSpy).toHaveBeenCalledTimes(1);
  });

  it('mentions the command and target in the warning so the call site is debuggable', () => {
    const body = 'x'.repeat(LINE_SIZE_WARN_THRESHOLD + 1);
    format('TAGMSG', ['nick123', body]);
    const msg = warnSpy.mock.calls[0]?.[0] as string;
    expect(msg).toContain('TAGMSG');
    expect(msg).toContain('nick123');
  });
});

describe('format() output correctness (regression guard)', () => {
  // Adding the warning logic mustn't change the formatted bytes —
  // tests below pin the wire output for several shapes.
  it('PRIVMSG with body and no tags renders cleanly', () => {
    // Body with a space → format() adds the `:` prefix per IRC convention.
    expect(format('PRIVMSG', ['#c', 'hi there'])).toBe('PRIVMSG #c :hi there');
    // Single-word body → no `:` prefix; attaches as a plain trailing param.
    expect(format('PRIVMSG', ['#c', 'hi'])).toBe('PRIVMSG #c hi');
  });

  it('TAGMSG with a single tag', () => {
    expect(format('TAGMSG', ['#c'], { foo: 'bar' })).toBe('@foo=bar TAGMSG #c');
  });

  it('PRIVMSG with multiple tags, body containing spaces', () => {
    expect(
      format('PRIVMSG', ['#c', 'hello world'], { a: '1', b: '2' }),
    ).toBe('@a=1;b=2 PRIVMSG #c :hello world');
  });
});
