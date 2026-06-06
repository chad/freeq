import { describe, it, expect } from 'vitest';
import {
  validatePersonaForm,
  buildPersonaRecord,
  personaRecordJson,
  type PersonaForm,
} from './personaRecord';

const base: PersonaForm = { name: 'Cassandra', systemPrompt: 'You foresee and warn.' };

describe('validatePersonaForm', () => {
  it('accepts a minimal valid form', () => {
    expect(validatePersonaForm(base)).toEqual([]);
  });

  it('requires name and system prompt', () => {
    const errs = validatePersonaForm({ name: '  ', systemPrompt: '' });
    expect(errs).toContain('Name is required.');
    expect(errs).toContain('System prompt is required.');
  });

  it('rejects a non-at:// forkedFrom and facePack', () => {
    expect(validatePersonaForm({ ...base, forkedFrom: 'oblivion' }))
      .toContain('Forked-from must be an at:// persona URI.');
    expect(validatePersonaForm({ ...base, facePack: 'https://x' }))
      .toContain('Face pack must be an at:// record URI.');
  });

  it('bounds speed', () => {
    expect(validatePersonaForm({ ...base, speed: 3 })).toContain('Speed must be between 0.5× and 2.0×.');
  });
});

describe('buildPersonaRecord', () => {
  it('produces a lexicon-shaped record', () => {
    const rec = buildPersonaRecord(
      {
        ...base,
        greeting: 'You won’t listen.',
        voiceId: 'abc',
        speed: 1.18,
        faceCharacter: 'oblivion',
        forkedFrom: 'at://did:plc:orig/at.freeq.persona/parent1',
      },
      '2026-06-06T00:00:00Z',
    );
    expect(rec).toEqual({
      $type: 'at.freeq.persona',
      name: 'Cassandra',
      systemPrompt: 'You foresee and warn.',
      greeting: 'You won’t listen.',
      voice: { provider: 'elevenlabs', voiceId: 'abc', speedMilli: 1180 },
      face: { character: 'oblivion' },
      forkedFrom: 'at://did:plc:orig/at.freeq.persona/parent1',
      createdAt: '2026-06-06T00:00:00Z',
    });
  });

  it('prefers a custom face pack over a built-in character', () => {
    const rec = buildPersonaRecord({
      ...base,
      faceCharacter: 'oblivion',
      facePack: 'at://did:plc:me/at.freeq.character/c1',
    }, 't');
    expect(rec.face).toEqual({ pack: 'at://did:plc:me/at.freeq.character/c1' });
  });

  it('omits optional sections when empty', () => {
    const rec = buildPersonaRecord(base, 't');
    expect(rec.voice).toBeUndefined();
    expect(rec.face).toBeUndefined();
    expect(rec.greeting).toBeUndefined();
    expect(rec.forkedFrom).toBeUndefined();
  });

  it('throws on an invalid form', () => {
    expect(() => buildPersonaRecord({ name: '', systemPrompt: '' })).toThrow(/invalid persona/);
  });

  it('round-trips through JSON pretty-print', () => {
    const rec = buildPersonaRecord(base, 't');
    expect(JSON.parse(personaRecordJson(rec))).toEqual(rec);
  });
});
