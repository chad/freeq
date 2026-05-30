#!/usr/bin/env node
/**
 * Production smoke test for freeq multiline (Phase 5).
 *
 * Connects two SDK clients (sender + receiver) to a live freeq server,
 * runs baseline + multi-line scenarios, and reports pass/fail. Exit
 * code is 0 if everything passes, 1 otherwise.
 *
 * Usage:
 *   # First, build the SDK so dist/ is current:
 *   ( cd freeq-sdk-js && npm run build )
 *
 *   # Then run against a server:
 *   node scripts/smoke-multiline.mjs
 *
 *   # Or with a different server / channel:
 *   FREEQ_URL=wss://irc.freeq.at/irc \
 *   FREEQ_CHANNEL='#my-smoke' \
 *     node scripts/smoke-multiline.mjs
 *
 * Env:
 *   FREEQ_URL      WebSocket URL (default wss://irc.zerosum.org/irc)
 *   FREEQ_CHANNEL  Test channel (default: a unique ephemeral channel
 *                  per run, `#smoke-<random>`, so repeat runs never
 *                  collide and no real channel gets polluted)
 *
 * What it tests:
 *   - A baseline: single-line PRIVMSG round-trip, edit, delete, reaction
 *   - B multiline: BATCH wire shape; inbound assembly into one message;
 *     edit/reply on multiline; large body (3 KB); ENC1 channel small +
 *     large (ciphertext-chunked) round-trip; legacy +freeq.at/multiline
 *     decode normalization
 *   - C signing: skipped in guest mode (prints a note explaining what
 *     would need to change to enable it)
 *
 * The script does NOT exercise: DID/SASL auth, S2S federation,
 * cross-server DMs. Those are out of scope for the deploy-smoke surface.
 */

import { FreeqClient } from '../freeq-sdk-js/dist/index.js';
import { setChannelKey } from '../freeq-sdk-js/dist/e2ee.js';

const URL = process.env.FREEQ_URL || 'wss://irc.zerosum.org/irc';
const STAMP = Date.now().toString(36);
// Default: ephemeral channel scoped to this run so reruns don't
// collide and #real-channels don't get polluted. Override via env if
// you want to watch the run live in a specific channel.
const CHANNEL = process.env.FREEQ_CHANNEL || `#smoke-${STAMP}`;
// All E2EE tests share one derived channel + passphrase. Derived from
// CHANNEL rather than reusing CHANNEL itself because setChannelKey()
// would silently encrypt the plaintext tests that come before.
const ENC1_CHANNEL = `${CHANNEL}-e2ee`;
const ENC1_PASS = `enc1-pass-${STAMP}`;
const SENDER_NICK = `smoke-tx-${STAMP}`;
const RECEIVER_NICK = `smoke-rx-${STAMP}`;

// ── Test harness ───────────────────────────────────────────────────

const results = [];
let currentName = '';
let currentFails = [];

async function runTest(name, fn) {
  currentName = name;
  currentFails = [];
  process.stdout.write(`… ${name}\r`);
  try {
    await fn();
    if (currentFails.length === 0) {
      console.log(`✓ ${name}`);
      results.push({ name, ok: true });
    } else {
      console.log(`✗ ${name}`);
      for (const f of currentFails) console.log(`    ${f}`);
      results.push({ name, ok: false, fails: currentFails });
    }
  } catch (e) {
    console.log(`✗ ${name} — threw: ${e?.message || e}`);
    results.push({ name, ok: false, fails: [`exception: ${e?.message || e}`] });
  }
  // Server enforces 5 msg / 2s flood protection per session. Sleep
  // enough to keep the sender's window clear between tests so we
  // don't get ERR_CANNOTSENDTOCHAN on a long run.
  await sleep(500);
}

function expect(cond, msg) {
  if (!cond) currentFails.push(msg);
}

function expectEq(actual, expected, msg) {
  if (actual !== expected) {
    currentFails.push(`${msg}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

// ── Client + wire helpers ──────────────────────────────────────────

async function makeClient(nick) {
  const client = new FreeqClient({
    url: URL,
    nick,
    skipInitialBrokerRefresh: true,
  });
  client.connect();
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${nick}: registration timed out (10s)`)), 10_000);
    client.once('registered', () => { clearTimeout(timer); resolve(); });
    client.once('authError', (e) => { clearTimeout(timer); reject(new Error(`${nick}: auth error — ${e}`)); });
  });
  return client;
}

async function joinAndWait(client, channel) {
  client.join(channel);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`join ${channel} timed out`)), 5000);
    client.once('channelJoined', () => { clearTimeout(timer); resolve(); });
  });
}

function captureSentFrames(client) {
  // Wrap the underlying ws send so we see exactly what hit the wire.
  // (The SDK's `raw` event is for INBOUND lines only.)
  const sent = [];
  // The SDK doesn't expose a sent-frame event; tap the transport's
  // send via a Proxy on `client.transport`. If the SDK ever changes,
  // this is the only thing to update. Fallback: parse from logs.
  const transport = /** @type {any} */ (client).transport;
  if (transport && typeof transport.send === 'function') {
    const orig = transport.send.bind(transport);
    transport.send = (line) => {
      sent.push(line);
      return orig(line);
    };
  }
  return sent;
}

function waitForMessage(client, channel, predicate, timeoutMs = 8000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      client.off('message', handler);
      reject(new Error(`timed out waiting for matching message on ${channel}`));
    }, timeoutMs);
    const handler = (ch, msg) => {
      if (ch.toLowerCase() === channel.toLowerCase() && predicate(msg)) {
        clearTimeout(timer);
        client.off('message', handler);
        resolve(msg);
      }
    };
    client.on('message', handler);
  });
}

function waitForEdit(client, channel, originalMsgid, timeoutMs = 8000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      client.off('messageEdited', handler);
      reject(new Error(`timed out waiting for edit of ${originalMsgid}`));
    }, timeoutMs);
    const handler = (ch, editOf, newText, newMsgid, isStreaming) => {
      if (ch.toLowerCase() === channel.toLowerCase() && editOf === originalMsgid) {
        clearTimeout(timer);
        client.off('messageEdited', handler);
        resolve({ newText, newMsgid, isStreaming });
      }
    };
    client.on('messageEdited', handler);
  });
}

async function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ── Setup ───────────────────────────────────────────────────────────

let sender, receiver, senderWire, receiverWire;

async function setup() {
  console.log(`\nfreeq multiline smoke`);
  console.log(`  url:     ${URL}`);
  console.log(`  channel: ${CHANNEL}`);
  console.log(`  sender:  ${SENDER_NICK}`);
  console.log(`  receiver: ${RECEIVER_NICK}\n`);

  sender = await makeClient(SENDER_NICK);
  receiver = await makeClient(RECEIVER_NICK);
  senderWire = captureSentFrames(sender);
  receiverWire = captureSentFrames(receiver);

  await joinAndWait(sender, CHANNEL);
  await joinAndWait(receiver, CHANNEL);
  // Give the server a beat to flush JOIN echoes to both ends.
  await sleep(500);
}

async function teardown() {
  try { sender?.quit('smoke done'); } catch {}
  try { receiver?.quit('smoke done'); } catch {}
  await sleep(200);
}

// ── Tests ──────────────────────────────────────────────────────────

async function tests() {
  // ── A baseline ───────────────────────────────────────────────────

  await runTest('A: single-line PRIVMSG round-trip', async () => {
    const text = `a1-${STAMP}`;
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text === text);
    expectEq(msg.from.toLowerCase(), SENDER_NICK.toLowerCase(), 'from');
    expectEq(msg.text, text, 'text');
    // Wire shape: NO BATCH frames for a single-line message
    const recentSent = senderWire.slice(-5).join('\n');
    expect(!recentSent.includes('BATCH'), 'no BATCH frames for single-line send');
  });

  await runTest('A: edit a single-line message in-place', async () => {
    const orig = `a2-orig-${STAMP}`;
    const edited = `a2-edited-${STAMP}`;
    sender.sendMessage(CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, CHANNEL, (m) => m.text === orig);
    expect(!!origMsg.id, 'original got a msgid');
    sender.sendEdit(CHANNEL, origMsg.id, edited);
    const ed = await waitForEdit(receiver, CHANNEL, origMsg.id);
    expectEq(ed.newText, edited, 'edit text');
  });

  await runTest('A: reply to a single-line message', async () => {
    const orig = `a-reply-orig-${STAMP}`;
    const replyText = `a-reply-reply-${STAMP}`;
    sender.sendMessage(CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, CHANNEL, (m) => m.text === orig);
    receiver.sendReply(CHANNEL, origMsg.id, replyText);
    const reply = await waitForMessage(sender, CHANNEL, (m) => m.text === replyText);
    expectEq(reply.replyTo, origMsg.id, 'replyTo points at original');
    expectEq(reply.text, replyText, 'reply text');
  });

  await runTest('A: delete own message', async () => {
    const text = `a-delete-${STAMP}`;
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text === text);
    expect(!!msg.id, 'original got a msgid');
    sender.sendDelete(CHANNEL, msg.id);
    let deleted = false;
    await new Promise((resolve) => {
      const handler = (ch, msgid) => {
        if (ch.toLowerCase() === CHANNEL.toLowerCase() && msgid === msg.id) {
          deleted = true;
          receiver.off('messageDeleted', handler);
          resolve();
        }
      };
      receiver.on('messageDeleted', handler);
      setTimeout(() => { receiver.off('messageDeleted', handler); resolve(); }, 3000);
    });
    expect(deleted, 'receiver observed messageDeleted for our msgid');
  });

  await runTest('A: reaction round-trip', async () => {
    const text = `a-react-${STAMP}`;
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text === text);
    receiver.sendReaction(CHANNEL, '👍', msg.id);
    let got = false;
    await new Promise((resolve) => {
      const handler = (_ch, msgid, emoji /*, fromNick*/) => {
        if (msgid === msg.id && emoji === '👍') {
          got = true;
          sender.off('reactionAdded', handler);
          resolve();
        }
      };
      sender.on('reactionAdded', handler);
      setTimeout(() => { sender.off('reactionAdded', handler); resolve(); }, 3000);
    });
    expect(got, 'sender observed reactionAdded');
  });

  await runTest('A: DM round-trip (single-line)', async () => {
    const text = `a-dm-${STAMP}`;
    sender.sendMessage(RECEIVER_NICK, text);
    const msg = await waitForMessage(receiver, SENDER_NICK, (m) => m.text === text);
    expectEq(msg.text, text, 'DM text matches');
    expectEq(msg.from.toLowerCase(), SENDER_NICK.toLowerCase(), 'DM from sender');
  });

  await runTest('A: long single-line message (~600 chars, no \\n)', async () => {
    const text = `a-long-${STAMP}-` + 'x'.repeat(600);
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith(`a-long-${STAMP}-`));
    expectEq(msg.text, text, 'long single-line text intact');
    // No BATCH for a single line, regardless of length
    const recentSent = senderWire.slice(-5).join('\n');
    expect(!recentSent.includes('BATCH'), 'no BATCH frames for long single-line');
  });

  await runTest('A: E2EE single-line round-trip (ENC1)', async () => {
    // All E2EE tests share ENC1_CHANNEL + ENC1_PASS. setChannelKey() is
    // idempotent (same pass = same key), so calling it once per test is
    // cheap. We use a derived channel rather than CHANNEL itself because
    // setChannelKey on CHANNEL would silently encrypt the later plaintext
    // tests too.
    await Promise.all([
      joinAndWait(sender, ENC1_CHANNEL),
      joinAndWait(receiver, ENC1_CHANNEL),
    ]);
    await sleep(300);
    await setChannelKey(ENC1_CHANNEL, ENC1_PASS);
    const text = `a-enc1-${STAMP}`;
    sender.sendMessage(ENC1_CHANNEL, text);
    const msg = await waitForMessage(receiver, ENC1_CHANNEL, (m) => m.text === text);
    expectEq(msg.text, text, 'decrypted plaintext matches');
    expect(msg.encrypted === true, 'flagged encrypted');
  });

  // ── B multiline (plaintext channel) ──────────────────────────────

  await runTest('B1: 3-line message → BATCH wire + assembled receive', async () => {
    const text = `b1-line1-${STAMP}\nb1-line2\nb1-line3`;
    const before = senderWire.length;
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith('b1-line1'));
    expectEq(msg.text, text, 'assembled text matches sent (real \\n)');
    expectEq(msg.text.split('\n').length, 3, 'three lines');
    // Outbound wire: BATCH +<id> draft/multiline, 3 chunks with batch=<id>, BATCH -<id>
    const sent = senderWire.slice(before);
    const opener = sent.find((l) => l.includes('BATCH +') && l.includes('draft/multiline'));
    const closer = sent.find((l) => /BATCH -\S+/.test(l));
    const chunks = sent.filter((l) => l.includes('PRIVMSG') && l.includes('batch='));
    expect(!!opener, 'wire contains BATCH +<id> draft/multiline');
    expect(!!closer, 'wire contains BATCH -<id>');
    expectEq(chunks.length, 3, 'three chunk PRIVMSGs');
  });

  await runTest('B3: ~3 KB multi-line paste', async () => {
    const paragraph = 'This is a long paragraph of text used to verify chunking behavior under realistic agent-debate load. '.repeat(8);
    const text = Array.from({ length: 4 }, (_, i) => `b3-${i + 1}: ${paragraph}`).join('\n');
    expect(text.length > 2500, `built ${text.length} chars`);
    sender.sendMessage(CHANNEL, text);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith('b3-1:'));
    expectEq(msg.text, text, 'large body round-trips exactly');
  });

  await runTest('B6: edit a multi-line message → full body arrives via BATCH', async () => {
    // Server's handle_edit now BATCH-wraps multi-line edits for
    // draft/multiline-capable receivers (which the JS SDK negotiates).
    // The receiver's SDK assembles the BATCH back into a single edit
    // event with the full multi-line newText. Fallback receivers
    // (no multiline cap) still see only line1 — they get a degraded
    // but wire-valid edit instead of malformed framing.
    const orig = `b6-orig-line1-${STAMP}\nb6-orig-line2\nb6-orig-line3`;
    const edited = `b6-edit-line1-${STAMP}\nb6-edit-line2`;
    sender.sendMessage(CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith('b6-orig-line1'));
    expect(!!origMsg.id, 'original multi-line got a msgid');
    sender.sendEdit(CHANNEL, origMsg.id, edited);
    const ed = await waitForEdit(receiver, CHANNEL, origMsg.id, 4000);
    expectEq(ed.newText, edited, 'multi-line edit text arrives intact');
  });

  await runTest('B7: reply to a multi-line message', async () => {
    const orig = `b7-orig-line1-${STAMP}\nb7-orig-line2`;
    sender.sendMessage(CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith('b7-orig-line1'));
    const replyText = `b7-reply-line1-${STAMP}\nb7-reply-line2`;
    receiver.sendReply(CHANNEL, origMsg.id, replyText);
    const reply = await waitForMessage(sender, CHANNEL, (m) => m.text.startsWith('b7-reply-line1'));
    expectEq(reply.text, replyText, 'reply text (multi-line)');
    expectEq(reply.replyTo, origMsg.id, 'replyTo points at original');
  });

  await runTest('B9: legacy +freeq.at/multiline → SDK decodes to real \\n', async () => {
    // Inject the wire form a pre-Phase-5 sender would have produced.
    // The SDK should decode `\\n` → real `\n` automatically.
    const text = `b9-line1-${STAMP}\\nb9-line2\\nb9-line3`;
    const transport = /** @type {any} */ (sender).transport;
    transport.send(`@+freeq.at/multiline= PRIVMSG ${CHANNEL} :${text}`);
    const expected = `b9-line1-${STAMP}\nb9-line2\nb9-line3`;
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith('b9-line1'));
    expectEq(msg.text, expected, 'SDK decoded literal \\\\n into real \\n');
    expectEq(msg.text.split('\n').length, 3, 'three lines');
  });

  await runTest('B10: codeblock content (```...```) round-trips byte-exact', async () => {
    // What freeq-app's MessageList parser sees as msg.text after SDK
    // normalization must contain real `\n` inside the codeblock so
    // <pre> renders the lines as the agent wrote them.
    const body = `b10-${STAMP}\n\`\`\`\nfn main() {\n    println!("hello");\n}\n\`\`\``;
    sender.sendMessage(CHANNEL, body);
    const msg = await waitForMessage(receiver, CHANNEL, (m) => m.text.startsWith(`b10-${STAMP}`));
    expectEq(msg.text, body, 'codeblock body round-trips with real \\n');
  });

  // ── B encrypted (ENC1, channel passphrase) ───────────────────────

  await runTest('B4: small multi-line in ENC1 channel (single PRIVMSG, no BATCH)', async () => {
    const text = `b4-line1-${STAMP}\nb4-line2\nb4-line3`;
    const before = senderWire.length;
    sender.sendMessage(ENC1_CHANNEL, text);
    const msg = await waitForMessage(receiver, ENC1_CHANNEL, (m) => m.text.startsWith('b4-line1'));
    expectEq(msg.text, text, 'plaintext recovered on receive');
    expect(msg.encrypted === true, 'message flagged encrypted');
    const sent = senderWire.slice(before);
    const batches = sent.filter((l) => l.includes('BATCH'));
    expectEq(batches.length, 0, 'no BATCH frames for small E2EE multiline');
    const encPriv = sent.find((l) => l.includes('+encrypted') && l.includes(`PRIVMSG ${ENC1_CHANNEL}`));
    expect(!!encPriv, 'one +encrypted PRIVMSG');
  });

  await runTest('B5: large multi-line in ENC1 channel → ciphertext-chunked BATCH', async () => {
    // ~8 KB plaintext → ~10.7 KB ciphertext after base64 → must chunk
    const paragraph = 'Wall of text to force ciphertext-chunking across multiline. '.repeat(20);
    const text = Array.from({ length: 6 }, (_, i) => `b5-${i + 1}: ${paragraph}`).join('\n');
    expect(text.length > 6000, `built ${text.length} chars plaintext`);

    const before = senderWire.length;
    sender.sendMessage(ENC1_CHANNEL, text);
    const msg = await waitForMessage(receiver, ENC1_CHANNEL, (m) => m.text.startsWith('b5-1:'), 12_000);
    expectEq(msg.text, text, 'large plaintext recovered on receive');
    expect(msg.encrypted === true, 'message flagged encrypted');

    const sent = senderWire.slice(before);
    const opener = sent.find((l) => l.includes('BATCH +') && l.includes('draft/multiline'));
    const closer = sent.find((l) => /BATCH -\S+/.test(l));
    const chunks = sent.filter((l) => l.includes('PRIVMSG') && l.includes('batch=') && l.includes('+encrypted'));
    expect(!!opener, 'BATCH opener present');
    expect(!!closer, 'BATCH closer present');
    expect(chunks.length >= 2, `at least 2 ciphertext chunks (got ${chunks.length})`);
    const concatCount = chunks.filter((l) => l.includes('+draft/multiline-concat')).length;
    expect(concatCount >= 1, `at least one chunk carries +draft/multiline-concat (got ${concatCount})`);
  });

  await runTest('B11: small E2EE edit (single-PRIVMSG ciphertext) round-trip', async () => {
    // Short E2EE edit body — ciphertext fits in one PRIVMSG, so the wire
    // shape is a single tagged PRIVMSG with +encrypted + +draft/edit.
    const orig = `b11-orig-${STAMP}`;
    sender.sendMessage(ENC1_CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, ENC1_CHANNEL, (m) => m.text === orig);
    expect(!!origMsg.id, 'orig E2EE msg has msgid');

    const edited = `b11-edit-${STAMP}`;
    sender.sendEdit(ENC1_CHANNEL, origMsg.id, edited);
    const ed = await waitForEdit(receiver, ENC1_CHANNEL, origMsg.id, 4000);
    expectEq(ed.newText, edited, 'small E2EE edit text arrives decrypted');
  });

  await runTest('B12: large E2EE edit (ciphertext-chunked BATCH) round-trip', async () => {
    // Plaintext large enough that the ciphertext exceeds one PRIVMSG MTU
    // so the sender BATCH-chunks the ciphertext (concat=true). With the
    // server fix carrying the sender's chunking through handle_edit,
    // multiline-capable receivers see a BATCH-wrapped edit and decrypt
    // the assembled ciphertext to recover the full new body.
    const paragraph = 'Wall of text to force ciphertext-chunking on the edit path. '.repeat(20);
    const orig = Array.from({ length: 6 }, (_, i) => `b12-orig-${i + 1}: ${paragraph}`).join('\n');
    sender.sendMessage(ENC1_CHANNEL, orig);
    const origMsg = await waitForMessage(receiver, ENC1_CHANNEL, (m) => m.text.startsWith('b12-orig-1:'), 12_000);
    expect(!!origMsg.id, 'orig large E2EE msg has msgid');

    const edited = Array.from({ length: 6 }, (_, i) => `b12-edit-${i + 1}: ${paragraph}`).join('\n');
    const before = senderWire.length;
    sender.sendEdit(ENC1_CHANNEL, origMsg.id, edited);
    const ed = await waitForEdit(receiver, ENC1_CHANNEL, origMsg.id, 12_000);
    expectEq(ed.newText, edited, 'large E2EE edit body arrives byte-exact');

    // Outbound wire from sender: BATCH +<id> with both +encrypted and
    // +draft/edit on the opener, chunked ciphertext, BATCH -<id>.
    const sent = senderWire.slice(before);
    const opener = sent.find((l) =>
      l.includes('BATCH +') && l.includes('draft/multiline') && l.includes('+draft/edit='),
    );
    expect(!!opener, 'edit BATCH opener carries +draft/edit + draft/multiline');
    const chunks = sent.filter(
      (l) => l.includes('PRIVMSG') && l.includes('batch=') && l.includes('+encrypted'),
    );
    expect(chunks.length >= 2, `at least 2 ciphertext-chunk PRIVMSGs (got ${chunks.length})`);
  });

  // ── C signing — guest mode skip ──────────────────────────────────

  await runTest('C: sig on BATCH opener (skipped in guest mode)', async () => {
    // Guest clients have no signing key, so this slice is informational.
    // To actually exercise it: provide a DID + session that mints an
    // ed25519 key via MSGSIG, send multi-line, and assert the opener
    // carries +freeq.at/sig=… and the server log shows no
    // "client signature verification failed" warning for our DID.
    console.log('    (guest mode — to test sigs, run with DID auth set up via the SDK)');
  });
}

// ── Main ───────────────────────────────────────────────────────────

async function main() {
  try {
    await setup();
    await tests();
  } finally {
    await teardown();
  }
  const passed = results.filter((r) => r.ok).length;
  const failed = results.length - passed;
  console.log(`\n${passed}/${results.length} passed${failed > 0 ? `  (${failed} failed)` : ''}`);
  process.exit(failed > 0 ? 1 : 0);
}

main().catch((e) => {
  console.error('smoke script crashed:', e);
  process.exit(2);
});
