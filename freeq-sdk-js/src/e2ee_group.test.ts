import { describe, it, expect } from 'vitest';
import {
  createGroup, rotate, encryptGroup, decryptGroup, sealFor, openSealed,
  sealedToWire, sealedFromWire, sealBatch, openBest, parseEpoch, isGroupEncrypted,
} from './e2ee_group';

describe('e2ee_group (EG1/EGK1)', () => {
  // A member holds an X25519 keypair. The private half stays a CryptoKey (the
  // browser path); the public half is raw bytes, as published in a pre-key
  // bundle and used by the steward to seal.
  async function pair(): Promise<{ priv: CryptoKey; pub: Uint8Array }> {
    const kp = await crypto.subtle.generateKey({ name: 'X25519' }, true, ['deriveBits']) as CryptoKeyPair;
    const pub = new Uint8Array(await crypto.subtle.exportKey('raw', kp.publicKey));
    return { priv: kp.privateKey, pub };
  }

  it('steward seals, member opens, reads; wrong member cannot', async () => {
    const g = createGroup('#eng');
    const alice = await pair();
    const mallory = await pair();

    const sealed = await sealFor(g, alice.pub);
    const msg = await encryptGroup(g, 'quarterly numbers');
    expect(msg.startsWith('EG1:1:')).toBe(true);
    expect(isGroupEncrypted(msg)).toBe(true);
    expect(parseEpoch(msg)).toBe(1);

    const aliceState = await openSealed(sealed, alice.priv);
    expect(aliceState).not.toBeNull();
    expect(await decryptGroup(aliceState!, msg)).toBe('quarterly numbers');

    // Mallory was never sealed to.
    expect(await openSealed(sealed, mallory.priv)).toBeNull();
  });

  it('sealed key survives wire round-trip', async () => {
    const g = createGroup('#secret-room');
    const bob = await pair();
    const sealed = await sealFor(g, bob.pub);
    const wire = sealedToWire(sealed);
    expect(wire.startsWith('EGK1:#secret-room:1:')).toBe(true);
    const parsed = sealedFromWire(wire)!;
    const state = await openSealed(parsed, bob.priv);
    expect(state).not.toBeNull();
    expect(state!.epoch).toBe(1);
  });

  it('rotation revokes the departed member (forward secrecy on membership change)', async () => {
    const e1 = createGroup('#eng');
    const alice = await pair();
    const bob = await pair();

    // Epoch 1: both members. Bob reads.
    const bobE1 = await openBest([[1, sealedToWire(await sealFor(e1, bob.pub))]], bob.priv);
    const m1 = await encryptGroup(e1, 'visible to bob');
    expect(await decryptGroup(bobE1!, m1)).toBe('visible to bob');

    // Bob leaves → rotate, re-seal only to Alice.
    const e2 = rotate(e1);
    expect(e2.epoch).toBe(2);
    const aliceE2 = await openBest([[2, sealedToWire(await sealFor(e2, alice.pub))]], alice.priv);
    const m2 = await encryptGroup(e2, 'post-offboarding secret');

    expect(await decryptGroup(aliceE2!, m2)).toBe('post-offboarding secret');
    // Bob still only holds epoch 1 → cannot read the epoch-2 message.
    expect(await decryptGroup(bobE1!, m2)).toBeNull();
  });

  it('sealBatch + openBest end to end', async () => {
    const g = createGroup('#eng');
    const alice = await pair();
    const bob = await pair();
    const batch = await sealBatch(g, [['did:plc:alice', alice.pub], ['did:plc:bob', bob.pub]]);
    expect(batch.length).toBe(2);

    // Simulate the GET response for Bob (server returns [epoch, sealed]).
    const bobWire = batch.find(([did]) => did === 'did:plc:bob')![1];
    const bobState = await openBest([[1, bobWire]], bob.priv);
    const msg = await encryptGroup(g, 'hello team');
    expect(await decryptGroup(bobState!, msg)).toBe('hello team');
  });
});
