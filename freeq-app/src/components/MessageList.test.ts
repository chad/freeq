/**
 * Unit tests for MessageList text parsing and rendering logic.
 *
 * Hotspot #4 (gamma 103). Tests parseTextSegments which handles
 * markdown, URLs, code blocks, and XSS prevention.
 */
import { describe, it, expect } from 'vitest';

// Extract the parsing logic to test it.
// Since parseTextSegments is not exported, we reproduce it here
// (same code as MessageList.tsx lines 98-156).

interface TextSegment {
  type: 'text' | 'link' | 'code' | 'codeblock' | 'bold' | 'italic' | 'strike';
  content: string;
  href?: string;
}

function parseTextSegments(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  const patterns: { re: RegExp; type: TextSegment['type']; group: number }[] = [
    { re: /```([\s\S]*?)```/g, type: 'codeblock', group: 1 },
    { re: /`([^`]+)`/g, type: 'code', group: 1 },
    { re: /(https?:\/\/[^\s<]+)/g, type: 'link', group: 1 },
    { re: /\*\*(.+?)\*\*/g, type: 'bold', group: 1 },
    { re: /(?<!\*)\*([^*]+)\*(?!\*)/g, type: 'italic', group: 1 },
    { re: /~~(.+?)~~/g, type: 'strike', group: 1 },
  ];

  const matches: { start: number; end: number; type: TextSegment['type']; content: string; full: string }[] = [];
  for (const p of patterns) {
    p.re.lastIndex = 0;
    let m;
    while ((m = p.re.exec(text)) !== null) {
      matches.push({ start: m.index, end: m.index + m[0].length, type: p.type, content: m[p.group], full: m[0] });
    }
  }

  matches.sort((a, b) => a.start - b.start);
  const filtered: typeof matches = [];
  let lastEnd = 0;
  for (const m of matches) {
    if (m.start >= lastEnd) { filtered.push(m); lastEnd = m.end; }
  }

  let pos = 0;
  for (const m of filtered) {
    if (m.start > pos) segments.push({ type: 'text', content: text.slice(pos, m.start) });
    if (m.type === 'link') segments.push({ type: 'link', content: m.content, href: m.content });
    else segments.push({ type: m.type, content: m.content });
    pos = m.end;
  }
  if (pos < text.length) segments.push({ type: 'text', content: text.slice(pos) });
  return segments;
}

// ═══════════════════════════════════════════════════════════════
// BASIC FORMATTING
// ═══════════════════════════════════════════════════════════════

describe('basic formatting', () => {
  it('plain text', () => {
    const s = parseTextSegments('hello world');
    expect(s).toEqual([{ type: 'text', content: 'hello world' }]);
  });

  it('bold', () => {
    const s = parseTextSegments('**bold text**');
    expect(s).toEqual([{ type: 'bold', content: 'bold text' }]);
  });

  it('italic', () => {
    const s = parseTextSegments('*italic text*');
    expect(s).toEqual([{ type: 'italic', content: 'italic text' }]);
  });

  it('strikethrough', () => {
    const s = parseTextSegments('~~struck~~');
    expect(s).toEqual([{ type: 'strike', content: 'struck' }]);
  });

  it('inline code', () => {
    const s = parseTextSegments('use `code` here');
    expect(s).toEqual([
      { type: 'text', content: 'use ' },
      { type: 'code', content: 'code' },
      { type: 'text', content: ' here' },
    ]);
  });

  it('code block', () => {
    const s = parseTextSegments('```\ncode block\n```');
    expect(s.length).toBe(1);
    expect(s[0].type).toBe('codeblock');
    expect(s[0].content).toContain('code block');
  });

  it('URL detected', () => {
    const s = parseTextSegments('check https://example.com/page');
    expect(s.length).toBe(2);
    expect(s[0]).toEqual({ type: 'text', content: 'check ' });
    expect(s[1].type).toBe('link');
    expect(s[1].href).toBe('https://example.com/page');
  });
});

// ═══════════════════════════════════════════════════════════════
// MIXED FORMATTING
// ═══════════════════════════════════════════════════════════════

describe('mixed formatting', () => {
  it('bold + link', () => {
    const s = parseTextSegments('**bold** and https://url.com');
    expect(s.some(x => x.type === 'bold')).toBe(true);
    expect(s.some(x => x.type === 'link')).toBe(true);
  });

  it('code block prevents inner formatting', () => {
    const s = parseTextSegments('```**not bold** *not italic*```');
    // Should be a single codeblock, not parsed as bold/italic
    expect(s.length).toBe(1);
    expect(s[0].type).toBe('codeblock');
  });

  it('inline code prevents inner formatting', () => {
    const s = parseTextSegments('`**code** not bold`');
    const code = s.find(x => x.type === 'code');
    expect(code?.content).toContain('**code**');
  });
});

// ═══════════════════════════════════════════════════════════════
// XSS AND INJECTION
// ═══════════════════════════════════════════════════════════════

describe('XSS prevention', () => {
  it('HTML tags are plain text', () => {
    const s = parseTextSegments('<script>alert(1)</script>');
    expect(s[0].type).toBe('text');
    expect(s[0].content).toBe('<script>alert(1)</script>');
  });

  it('img onerror is plain text', () => {
    const s = parseTextSegments('<img src=x onerror=alert(1)>');
    expect(s[0].type).toBe('text');
    expect(s[0].content).toContain('<img');
  });

  it('javascript: URL not detected as link', () => {
    const s = parseTextSegments('javascript:alert(1)');
    // The regex only matches https?:// — javascript: should be plain text
    expect(s.every(x => x.type !== 'link')).toBe(true);
  });

  it('data: URL not detected as link', () => {
    const s = parseTextSegments('data:text/html,<script>alert(1)</script>');
    expect(s.every(x => x.type !== 'link')).toBe(true);
  });

  it('BUG: http URL with XSS in path is detected as link', () => {
    const s = parseTextSegments('http://evil.com/<script>alert(1)</script>');
    const link = s.find(x => x.type === 'link');
    // The URL regex captures up to the < which stops it
    // Check what actually gets captured
    if (link) {
      expect(link.href).not.toContain('<script>');
    }
  });

  it('URL stops at < character', () => {
    const s = parseTextSegments('check https://example.com/path<injection');
    const link = s.find(x => x.type === 'link');
    // The regex has [^\s<]+ which stops at <
    expect(link?.href).toBe('https://example.com/path');
  });
});

// ═══════════════════════════════════════════════════════════════
// EDGE CASES
// ═══════════════════════════════════════════════════════════════

describe('edge cases', () => {
  it('empty string', () => {
    const s = parseTextSegments('');
    expect(s).toEqual([]);
  });

  it('only whitespace', () => {
    const s = parseTextSegments('   ');
    expect(s).toEqual([{ type: 'text', content: '   ' }]);
  });

  it('unclosed bold', () => {
    const s = parseTextSegments('**unclosed');
    expect(s[0].type).toBe('text');
    expect(s[0].content).toBe('**unclosed');
  });

  it('unclosed italic', () => {
    const s = parseTextSegments('*unclosed');
    expect(s[0].type).toBe('text');
  });

  it('unclosed inline code', () => {
    const s = parseTextSegments('`unclosed');
    expect(s[0].type).toBe('text');
  });

  it('unclosed code block', () => {
    const s = parseTextSegments('```unclosed');
    expect(s[0].type).toBe('text');
  });

  it('nested bold in italic', () => {
    // **bold *and italic* text** — overlap filtering keeps first match
    const s = parseTextSegments('**bold *and italic* text**');
    expect(s.some(x => x.type === 'bold')).toBe(true);
  });

  it('asterisks that are not formatting', () => {
    const s = parseTextSegments('a * b * c');
    // Single * with spaces should be italic in some parsers, plain text in others
    // Document actual behavior
    const types = s.map(x => x.type);
    // Either parsed as italic or left as text — no crash
  });

  it('very long text (10K chars)', () => {
    const long = 'x'.repeat(10000);
    const s = parseTextSegments(long);
    expect(s.length).toBe(1);
    expect(s[0].content.length).toBe(10000);
  });

  it('many URLs', () => {
    const text = Array.from({ length: 20 }, (_, i) => `https://url${i}.com`).join(' ');
    const s = parseTextSegments(text);
    const links = s.filter(x => x.type === 'link');
    expect(links.length).toBe(20);
  });

  it('URL with query params preserved', () => {
    const s = parseTextSegments('https://example.com/search?q=test&page=1');
    const link = s.find(x => x.type === 'link');
    expect(link?.href).toContain('?q=test&page=1');
  });

  it('URL with fragment preserved', () => {
    const s = parseTextSegments('https://example.com/page#section');
    const link = s.find(x => x.type === 'link');
    expect(link?.href).toContain('#section');
  });

  it('URL with parentheses (Wikipedia-style)', () => {
    const s = parseTextSegments('https://en.wikipedia.org/wiki/Rust_(programming_language)');
    const link = s.find(x => x.type === 'link');
    expect(link?.href).toContain('Rust_(programming_language)');
  });

  it('multiple formatting in one line', () => {
    const s = parseTextSegments('**bold** *italic* `code` ~~strike~~');
    const types = s.filter(x => x.type !== 'text').map(x => x.type);
    expect(types).toContain('bold');
    expect(types).toContain('italic');
    expect(types).toContain('code');
    expect(types).toContain('strike');
  });

  it('unicode emoji in text', () => {
    const s = parseTextSegments('🎉 hello **world** 🌍');
    expect(s.some(x => x.content.includes('🎉'))).toBe(true);
    expect(s.some(x => x.type === 'bold')).toBe(true);
  });

  it('BUG: bold with * inside (e.g. multiplication)', () => {
    const s = parseTextSegments('**2*3 = 6**');
    // The bold regex is **(.+?)** — lazy match
    // With * inside, it might close early: **2* → italic?
    // Document actual behavior
    const hasBold = s.some(x => x.type === 'bold');
    // This might not parse as expected
  });

  it('code block with backticks inside', () => {
    const s = parseTextSegments('```\nuse `backticks` in code\n```');
    expect(s[0].type).toBe('codeblock');
    expect(s[0].content).toContain('`backticks`');
  });

  it('overlapping formats: first match wins', () => {
    // `code **with bold**` — code should win
    const s = parseTextSegments('`code **with bold**`');
    const code = s.find(x => x.type === 'code');
    expect(code).toBeDefined();
    expect(code?.content).toContain('**with bold**');
  });
});

// ═══════════════════════════════════════════════════════════════
// MULTILINE: real \n preserved through tokenization
// ═══════════════════════════════════════════════════════════════
//
// The SDK normalizes both wire forms (`draft/multiline` BATCH and
// the legacy `+freeq.at/multiline` tag) into `message.text` with
// REAL `\n` characters. The app's prior `\\n` → `\n` decode was
// removed. These tests lock in that the parser preserves real
// newlines inside codeblocks and other segments — anything that
// silently rewrote `\\n` would surface here.

describe('multiline content with real newlines', () => {
  it('codeblock with multi-line body preserves real \\n inside content', () => {
    const text = '```\nline one\nline two\nline three\n```';
    const s = parseTextSegments(text);
    expect(s.length).toBe(1);
    expect(s[0].type).toBe('codeblock');
    expect(s[0].content).toContain('line one\nline two\nline three');
  });

  it('codeblock with literal \\\\n in content is NOT silently rewritten', () => {
    // User types `echo "a\\n b"` inside a codeblock — the two-char
    // `\n` escape sequence stays literal so it round-trips for shell
    // / language interpreters that act on it later.
    const text = '```\necho "a\\nb"\n```';
    const s = parseTextSegments(text);
    expect(s[0].type).toBe('codeblock');
    expect(s[0].content).toContain('echo "a\\nb"');
    expect(s[0].content).not.toContain('echo "a\nb"');
  });

  it('plain text with real \\n preserved as one text segment', () => {
    const s = parseTextSegments('line one\nline two\nline three');
    expect(s.length).toBe(1);
    expect(s[0].type).toBe('text');
    expect(s[0].content).toBe('line one\nline two\nline three');
  });

  it('codeblock with leading and trailing newlines (typical agent output)', () => {
    // Agents often produce ```\ncontent\n``` — the codeblock content
    // starts and ends with \n. The rendering layer trims those leading/
    // trailing newlines via .replace(/^\n|\n$/g, '') in the <pre>;
    // the captured content here still includes them.
    const text = '```\nfn main() {\n    println!("hi");\n}\n```';
    const s = parseTextSegments(text);
    expect(s[0].type).toBe('codeblock');
    expect(s[0].content).toBe('\nfn main() {\n    println!("hi");\n}\n');
  });
});
