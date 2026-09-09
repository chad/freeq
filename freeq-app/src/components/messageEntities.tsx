/**
 * @nick and #channel entities, shared by the plain-text renderer
 * (MessageList) and the markdown renderer (MarkdownRenderer) so both
 * paths tokenise and render them the same way.
 */
import { useStore } from '../store';
import { joinChannel } from '../irc/client';
import type { RowEvidence } from './UserPopover';

/** @nick — not preceded by a word char, so me@example.com is not a mention. */
export const MENTION_RE = /(?<![A-Za-z0-9])@([A-Za-z0-9][A-Za-z0-9._-]*)/g;
/** #channel — not preceded by a word char, a slash, or another #. */
export const CHANNEL_RE = /(?<![\w/#])#([A-Za-z0-9][A-Za-z0-9._-]*)/g;

/** Context for making @nick / #channel spans interactive. */
export interface RenderCtx {
  channel?: string;
  onNickClick?: (nick: string, did: string | undefined, origin: string | undefined, e: React.MouseEvent, evidence?: RowEvidence) => void;
}

/** A clickable @nick. `display` is what the user typed, `nick` the bare name. */
export function MentionSpan({ display, nick, ctx }: { display: string; nick: string; ctx?: RenderCtx }) {
  return (
    <button
      type="button"
      className="text-accent hover:underline font-medium"
      onClick={(e) => {
        e.stopPropagation();
        // Resolve DID from the channel roster (impersonation-safe).
        const did = ctx?.channel
          ? useStore.getState().channels.get(ctx.channel.toLowerCase())?.members.get(nick.toLowerCase())?.did
          : undefined;
        ctx?.onNickClick?.(nick, did, undefined, e);
      }}
    >{display}</button>
  );
}

/** A clickable #channel. `name` includes the leading '#'. */
export function ChannelSpan({ name }: { name: string }) {
  return (
    <button
      type="button"
      className="text-accent hover:underline font-medium"
      onClick={(e) => { e.stopPropagation(); joinChannel(name); }}
    >{name}</button>
  );
}

export interface EntitySegment {
  type: 'text' | 'mention' | 'channel';
  /** Text as written (mentions keep their '@', channels their '#'). */
  content: string;
  /** mention: the bare nick. channel: the '#name'. */
  value?: string;
}

/** Split a run of plain text into text / mention / channel segments. */
export function splitEntities(text: string): EntitySegment[] {
  const matches: { start: number; end: number; type: 'mention' | 'channel'; full: string; value: string }[] = [];
  for (const [re, type] of [[MENTION_RE, 'mention'], [CHANNEL_RE, 'channel']] as const) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(text)) !== null) {
      matches.push({
        start: m.index,
        end: m.index + m[0].length,
        type,
        full: m[0],
        value: type === 'mention' ? m[1] : m[0],
      });
    }
  }
  matches.sort((a, b) => a.start - b.start);

  const segments: EntitySegment[] = [];
  let pos = 0;
  for (const m of matches) {
    if (m.start < pos) continue;
    if (m.start > pos) segments.push({ type: 'text', content: text.slice(pos, m.start) });
    segments.push({ type: m.type, content: m.full, value: m.value });
    pos = m.end;
  }
  if (pos < text.length) segments.push({ type: 'text', content: text.slice(pos) });
  return segments;
}
