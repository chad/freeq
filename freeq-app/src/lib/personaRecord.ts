// Build and validate `at.freeq.persona` records — the public, signed
// artifact a persona becomes in a PDS. Pure (no I/O) so it's trivially
// testable; the Persona Studio uses it for live preview + export, and a
// PDS write (OAuth) eventually publishes the result.

export interface PersonaVoice {
  provider: string;
  voiceId: string;
  /** Speed multiplier ×1000 (e.g. 1180 = 1.18×) — integer per lexicon. */
  speedMilli?: number;
}

export interface PersonaFace {
  /** Built-in ghostly character name. */
  character?: string;
  /** Or an at.freeq.character record URI for a custom face. */
  pack?: string;
}

export interface PersonaRecord {
  $type: 'at.freeq.persona';
  name: string;
  systemPrompt: string;
  greeting?: string;
  voice?: PersonaVoice;
  face?: PersonaFace;
  /** The parent persona's at:// URI — makes lineage intrinsic. */
  forkedFrom?: string;
  createdAt: string;
}

/** The editable shape the Studio form binds to. */
export interface PersonaForm {
  name: string;
  systemPrompt: string;
  greeting?: string;
  voiceId?: string;
  /** Human-facing speed multiplier, e.g. 1.18. */
  speed?: number;
  faceCharacter?: string;
  facePack?: string;
  forkedFrom?: string;
}

export const BUILTIN_CHARACTERS = ['eliza', 'oblivion', 'narrator', 'utopia'] as const;

const AT_URI_RE = /^at:\/\/[^/]+\/[^/]+\/[^/]+$/;

/** Return a list of human-readable validation errors ([] = valid). */
export function validatePersonaForm(f: PersonaForm): string[] {
  const errors: string[] = [];
  if (!f.name?.trim()) errors.push('Name is required.');
  if ((f.name?.trim().length ?? 0) > 128) errors.push('Name must be ≤ 128 characters.');
  if (!f.systemPrompt?.trim()) errors.push('System prompt is required.');
  if (f.facePack && !AT_URI_RE.test(f.facePack.trim())) {
    errors.push('Face pack must be an at:// record URI.');
  }
  if (f.forkedFrom && !AT_URI_RE.test(f.forkedFrom.trim())) {
    errors.push('Forked-from must be an at:// persona URI.');
  }
  if (f.speed != null && (f.speed < 0.5 || f.speed > 2.0)) {
    errors.push('Speed must be between 0.5× and 2.0×.');
  }
  return errors;
}

/**
 * Build a lexicon-shaped persona record from the form. Throws if the
 * form is invalid (call {@link validatePersonaForm} first to surface
 * errors in the UI). `createdAt` is injectable for deterministic tests.
 */
export function buildPersonaRecord(f: PersonaForm, createdAt?: string): PersonaRecord {
  const errors = validatePersonaForm(f);
  if (errors.length) throw new Error(`invalid persona: ${errors.join(' ')}`);

  const rec: PersonaRecord = {
    $type: 'at.freeq.persona',
    name: f.name.trim(),
    systemPrompt: f.systemPrompt.trim(),
    createdAt: createdAt ?? new Date().toISOString(),
  };
  if (f.greeting?.trim()) rec.greeting = f.greeting.trim();

  if (f.voiceId?.trim()) {
    rec.voice = { provider: 'elevenlabs', voiceId: f.voiceId.trim() };
    if (f.speed != null) rec.voice.speedMilli = Math.round(f.speed * 1000);
  }

  const face: PersonaFace = {};
  if (f.facePack?.trim()) face.pack = f.facePack.trim();
  else if (f.faceCharacter?.trim()) face.character = f.faceCharacter.trim();
  if (face.character || face.pack) rec.face = face;

  if (f.forkedFrom?.trim()) rec.forkedFrom = f.forkedFrom.trim();

  return rec;
}

/** Pretty-print a record for the Studio's preview / copy / download. */
export function personaRecordJson(rec: PersonaRecord): string {
  return JSON.stringify(rec, null, 2);
}
