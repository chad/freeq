// Default mention matcher used by FreeqBot.checkMention. Callers who want
// different addressing semantics pass their own matcher via the bot's
// `mention.matcher` config — this is just the shipped default.
//
// Default policy:
//   - "@<nick>" anywhere, preceded by start-of-string or whitespace, with
//     <nick> as a complete word. (So "email@nick.com" does not match.)
//   - "<nick>:" or "<nick>,", anywhere, preceded by start-of-string or
//     whitespace, with <nick> as a complete word.
//   - Bare "<nick>" as a standalone word with no @ or :/, does NOT match.
//     Third-person references like "yokota wrote a great thing" are
//     conversation about the bot, not addressing it.
//
// Returns the message text with the addressing token stripped (and
// surrounding whitespace collapsed), or null if not addressed.

/** Escape a string for safe inclusion in a `RegExp`. */
function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The default `matcher` for FreeqBot.checkMention. Returns the message
 *  text with the addressing prefix stripped, or null if the bot was not
 *  addressed under the default rules. */
export function matchMention(nick: string, text: string): { stripped: string } | null {
  if (!nick || !text) return null;
  const escaped = escapeRegex(nick);
  // Two acceptable forms, both anchored at start-of-string or whitespace:
  //   - @<nick>  followed by a word boundary
  //   - <nick>   followed by : or ,
  const re = new RegExp(
    `(?:^|\\s)(?:@${escaped}\\b|${escaped}[:,])\\s*`,
    "i",
  );
  const m = re.exec(text);
  if (!m) return null;
  // Replace the match with a single space and collapse runs of whitespace.
  const stripped = text.replace(re, " ").replace(/\s+/g, " ").trim();
  return { stripped };
}
