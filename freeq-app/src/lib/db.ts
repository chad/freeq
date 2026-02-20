/**
 * IndexedDB persistence for client-side state.
 * Stores: last-read msgid per channel, user preferences, recent channels.
 */
import { openDB, type IDBPDatabase } from 'idb';

const DB_NAME = 'freeq';
const DB_VERSION = 1;

let dbPromise: Promise<IDBPDatabase> | null = null;

function getDb() {
  if (!dbPromise) {
    dbPromise = openDB(DB_NAME, DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains('readState')) {
          db.createObjectStore('readState');
        }
        if (!db.objectStoreNames.contains('preferences')) {
          db.createObjectStore('preferences');
        }
        if (!db.objectStoreNames.contains('recentChannels')) {
          db.createObjectStore('recentChannels');
        }
      },
    });
  }
  return dbPromise;
}

// ── Read state ──

export async function getLastReadMsgId(channel: string): Promise<string | null> {
  const db = await getDb();
  return (await db.get('readState', channel.toLowerCase())) || null;
}

export async function setLastReadMsgId(channel: string, msgId: string): Promise<void> {
  const db = await getDb();
  await db.put('readState', msgId, channel.toLowerCase());
}

// ── Preferences ──

export type Theme = 'dark' | 'light';
export type Density = 'comfortable' | 'compact';

export interface Preferences {
  theme: Theme;
  density: Density;
  notifications: boolean;
  sounds: boolean;
  fontSize: number;
}

const DEFAULT_PREFS: Preferences = {
  theme: 'dark',
  density: 'comfortable',
  notifications: true,
  sounds: true,
  fontSize: 15,
};

export async function getPreferences(): Promise<Preferences> {
  const db = await getDb();
  const stored = await db.get('preferences', 'prefs');
  return { ...DEFAULT_PREFS, ...stored };
}

export async function setPreferences(prefs: Partial<Preferences>): Promise<void> {
  const db = await getDb();
  const current = await getPreferences();
  await db.put('preferences', { ...current, ...prefs }, 'prefs');
}

// ── Recent channels ──

export async function getRecentChannels(): Promise<string[]> {
  const db = await getDb();
  return (await db.get('recentChannels', 'list')) || [];
}

export async function addRecentChannel(channel: string): Promise<void> {
  const db = await getDb();
  const list: string[] = (await db.get('recentChannels', 'list')) || [];
  const filtered = list.filter((c) => c.toLowerCase() !== channel.toLowerCase());
  filtered.unshift(channel);
  await db.put('recentChannels', filtered.slice(0, 50), 'list');
}
