// @vitest-environment jsdom
/**
 * Chat markdown keeps the line breaks the sender typed, and renders
 * @nick / #channel the way the plain-text path does.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import { MarkdownMessage } from './MarkdownRenderer';

afterEach(cleanup);

describe('MarkdownMessage', () => {
  it('renders a line break for each single newline in a paragraph', () => {
    const { container } = render(
      <MarkdownMessage text={'Commands:\n!help - show help\n!status - show status'} />
    );
    expect(container.querySelectorAll('p')).toHaveLength(1);
    expect(container.querySelectorAll('br')).toHaveLength(2);
  });

  it('keeps blank-line-separated text in separate paragraphs', () => {
    const { container } = render(<MarkdownMessage text={'first\n\nsecond'} />);
    expect(container.querySelectorAll('p')).toHaveLength(2);
    expect(container.querySelectorAll('br')).toHaveLength(0);
  });

  it('keeps newlines inside a fenced code block as text, not breaks', () => {
    const { container } = render(
      <MarkdownMessage text={'```sh\nfoo\nbar\n```'} />
    );
    const code = container.querySelector('pre code');
    expect(code?.querySelectorAll('br')).toHaveLength(0);
    expect(code?.textContent).toBe('foo\nbar\n');
  });

  it('still renders GFM tables', () => {
    const { container } = render(
      <MarkdownMessage text={'| a | b |\n| - | - |\n| 1 | 2 |'} />
    );
    expect(container.querySelector('table')).not.toBeNull();
    expect(container.querySelectorAll('td')).toHaveLength(2);
  });

  // @nick / #channel — same spans the plain-text path renders
  // (MessageList's 'mention' and 'channel' segments: a
  // `button.text-accent.hover:underline.font-medium` holding the token).
  const entities = (c: HTMLElement) =>
    [...c.querySelectorAll('button.text-accent.font-medium')].map((b) => b.textContent);

  it('renders a mention and a channel link in markdown text', () => {
    const { container } = render(<MarkdownMessage text="hello @alice and #general" />);
    expect(entities(container)).toEqual(['@alice', '#general']);
  });

  it('leaves a mention inside a code span alone', () => {
    const { container } = render(<MarkdownMessage text="try `@alice` here" />);
    expect(entities(container)).toEqual([]);
    expect(container.querySelector('code')?.textContent).toBe('@alice');
  });

  it('leaves a mention inside a fenced code block alone', () => {
    const { container } = render(<MarkdownMessage text={'```sh\nping @alice\n```'} />);
    expect(entities(container)).toEqual([]);
    expect(container.querySelector('pre code')?.textContent).toBe('ping @alice\n');
  });

  it('does not treat an email address as a mention', () => {
    const { container } = render(<MarkdownMessage text="write to me@example.com" />);
    expect(entities(container)).toEqual([]);
  });
});
