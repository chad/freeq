/** Parsed IRC message with optional IRCv3 tags. */
export interface IRCMessage {
  tags: Record<string, string>;
  prefix: string;
  command: string;
  params: string[];
}

/** Extract nick from a prefix like "nick!user@host" */
export function prefixNick(prefix: string): string {
  const i = prefix.indexOf('!');
  return i > 0 ? prefix.slice(0, i) : prefix;
}

/** Parse a raw IRC line into a structured message. */
export function parse(raw: string): IRCMessage {
  let line = raw.replace(/\r?\n$/, '');
  const tags: Record<string, string> = {};

  // Parse tags (@key=value;key2=value2)
  if (line.startsWith('@')) {
    const sp = line.indexOf(' ');
    const tagStr = line.substring(1, sp);
    line = line.slice(sp + 1).trimStart();
    for (const t of tagStr.split(';')) {
      const eq = t.indexOf('=');
      if (eq >= 0) {
        tags[t.slice(0, eq)] = t
          .slice(eq + 1)
          .replace(/\\s/g, ' ')
          .replace(/\\:/g, ';')
          .replace(/\\\\/g, '\\')
          .replace(/\\r/g, '\r')
          .replace(/\\n/g, '\n');
      } else {
        tags[t] = '';
      }
    }
  }

  // Parse prefix (:server or :nick!user@host)
  let prefix = '';
  if (line.startsWith(':')) {
    const sp = line.indexOf(' ');
    prefix = line.slice(1, sp);
    line = line.slice(sp + 1);
  }

  // Parse command and params
  const params: string[] = [];
  while (line.length) {
    if (line.startsWith(':')) {
      params.push(line.slice(1));
      break;
    }
    const sp = line.indexOf(' ');
    if (sp < 0) {
      params.push(line);
      break;
    }
    params.push(line.slice(0, sp));
    line = line.slice(sp + 1);
  }

  const command = (params.shift() || '').toUpperCase();
  return { tags, prefix, command, params };
}

/** Serialize tags to IRC tag string (without @). */
function serializeTags(tags: Record<string, string>): string {
  return Object.entries(tags)
    .map(([k, v]) => {
      const escaped = v
        .replace(/\\/g, '\\\\')
        .replace(/;/g, '\\:')
        .replace(/ /g, '\\s')
        .replace(/\r/g, '\\r')
        .replace(/\n/g, '\\n');
      return escaped ? `${k}=${escaped}` : k;
    })
    .join(';');
}

/** Format a raw IRC line from parts. */
export function format(
  command: string,
  params: string[],
  tags?: Record<string, string>,
): string {
  let line = '';
  if (tags && Object.keys(tags).length > 0) {
    line += `@${serializeTags(tags)} `;
  }
  line += command;
  for (let i = 0; i < params.length; i++) {
    if (i === params.length - 1 && (params[i].includes(' ') || params[i].startsWith(':'))) {
      line += ` :${params[i]}`;
    } else {
      line += ` ${params[i]}`;
    }
  }
  return line;
}
