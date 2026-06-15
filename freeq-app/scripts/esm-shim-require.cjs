/**
 * Node 18 compatibility shims for vitest fork workers.
 *
 * Three issues prevent jsdom v29 from loading in node 18 fork workers:
 *
 * 1. @exodus/bytes/encoding-lite.js is pure ESM.
 *    jsdom → html-encoding-sniffer synchronously require()s it, which raises
 *    ERR_REQUIRE_ESM.  Fix: intercept require() via Module._resolveFilename
 *    and redirect to our CJS shim (encoding-lite-cjs.cjs).
 *
 * 2. webidl-conversions reads ArrayBuffer.prototype.resizable (added in
 *    node 20 / V8 10.4).  On node 18 the property doesn't exist, so
 *    getOwnPropertyDescriptor returns undefined and .get throws TypeError.
 *    Fix: polyfill the property before webidl-conversions is required.
 *
 * 3. webidl-conversions reads SharedArrayBuffer.prototype.growable (also
 *    added in node 20).  Same fix.
 *
 * This file is loaded via --require in vitest.config.ts → test.execArgv so
 * it runs in every fork worker process before any test code.
 */
'use strict';

// ── 1. ArrayBuffer.prototype.resizable polyfill (node 18) ─────────────────
if (
  typeof ArrayBuffer !== 'undefined' &&
  !Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable')
) {
  Object.defineProperty(ArrayBuffer.prototype, 'resizable', {
    // Node 18 ArrayBuffers are never resizable; always return false.
    get() { return false; },
    configurable: true,
    enumerable: false,
  });
}

// ── 2. SharedArrayBuffer.prototype.growable polyfill (node 18) ────────────
if (
  typeof SharedArrayBuffer !== 'undefined' &&
  !Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'growable')
) {
  Object.defineProperty(SharedArrayBuffer.prototype, 'growable', {
    // Node 18 SharedArrayBuffers are never growable; always return false.
    get() { return false; },
    configurable: true,
    enumerable: false,
  });
}

// ── 3. @exodus/bytes/encoding-lite.js → CJS redirect ─────────────────────
const Module = require('module');
const path = require('path');

const SHIM_TARGET = path.join(__dirname, 'encoding-lite-cjs.cjs');

const original = Module._resolveFilename.bind(Module);
Module._resolveFilename = function (request, parent, isMain, options) {
  // Match any require that ends with @exodus/bytes/encoding-lite.js
  if (request.includes('@exodus/bytes') && request.includes('encoding-lite')) {
    return SHIM_TARGET;
  }
  return original(request, parent, isMain, options);
};
