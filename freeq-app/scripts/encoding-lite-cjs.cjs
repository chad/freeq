/**
 * CJS shim for @exodus/bytes/encoding-lite.js (pure-ESM on node 18).
 *
 * html-encoding-sniffer v6 only uses getBOMEncoding and labelToName.
 * This re-implements both from the WHATWG Encoding spec so tests on node 18
 * don't crash with ERR_REQUIRE_ESM when jsdom tries to load html-encoding-sniffer.
 *
 * Source of truth: https://encoding.spec.whatwg.org/#names-and-labels
 */
'use strict';

// ── BOM detection ──────────────────────────────────────────────────────────

/**
 * Return the BOM-detected encoding name or null.
 * @param {Uint8Array} input
 * @returns {'utf-8' | 'utf-16le' | 'utf-16be' | null}
 */
function getBOMEncoding(input) {
  const u8 = input instanceof Uint8Array ? input : new Uint8Array(input);
  if (u8.length >= 3 && u8[0] === 0xef && u8[1] === 0xbb && u8[2] === 0xbf) return 'utf-8';
  if (u8.length < 2) return null;
  if (u8[0] === 0xff && u8[1] === 0xfe) return 'utf-16le';
  if (u8[0] === 0xfe && u8[1] === 0xff) return 'utf-16be';
  return null;
}

// ── Label → canonical name ─────────────────────────────────────────────────
// Condensed alias table from https://encoding.spec.whatwg.org/#names-and-labels
// Only the aliases that html-encoding-sniffer realistically encounters are
// listed; this is NOT an exhaustive WHATWG table.

const LABEL_MAP = buildLabelMap();

function buildLabelMap() {
  const map = new Map();

  const add = (canonical, ...aliases) => {
    map.set(canonical.toLowerCase(), canonical);
    for (const a of aliases) map.set(a.toLowerCase(), canonical);
  };

  add('UTF-8', 'utf8', 'unicode-1-1-utf-8', 'utf-8');
  add('UTF-16LE', 'utf-16', 'utf-16le', 'unicode', 'csunicode', 'ucs-2', 'iso-10646-ucs-2');
  add('UTF-16BE', 'utf-16be', 'unicodefffe');
  add('windows-1252',
    'ansi_x3.4-1968', 'ascii', 'cp1252', 'cp819', 'csisolatin1', 'ibm819',
    'iso-8859-1', 'iso-ir-100', 'iso8859-1', 'iso88591', 'iso_8859-1',
    'iso_8859-1:1987', 'l1', 'latin1', 'us-ascii', 'windows-1252', 'x-cp1252');
  add('windows-1251', 'cp1251', 'windows-1251', 'x-cp1251');
  add('windows-1250', 'cp1250', 'windows-1250', 'x-cp1250');
  add('windows-1253', 'cp1253', 'windows-1253', 'x-cp1253');
  add('windows-1254',
    'cp1254', 'csisolatin5', 'iso-8859-9', 'iso-ir-148', 'iso8859-9',
    'iso88599', 'iso_8859-9', 'iso_8859-9:1989', 'l5', 'latin5', 'windows-1254', 'x-cp1254');
  add('windows-1255', 'cp1255', 'windows-1255', 'x-cp1255');
  add('windows-1256', 'cp1256', 'windows-1256', 'x-cp1256');
  add('windows-1257', 'cp1257', 'windows-1257', 'x-cp1257');
  add('windows-1258', 'cp1258', 'windows-1258', 'x-cp1258');
  add('windows-874', 'dos-874', 'iso-8859-11', 'iso8859-11', 'iso885911', 'tis-620', 'windows-874');
  add('ISO-8859-2',
    'csisolatin2', 'iso-8859-2', 'iso-ir-101', 'iso8859-2', 'iso88592',
    'iso_8859-2', 'iso_8859-2:1987', 'l2', 'latin2');
  add('ISO-8859-3',
    'csisolatin3', 'iso-8859-3', 'iso-ir-109', 'iso8859-3', 'iso88593',
    'iso_8859-3', 'iso_8859-3:1988', 'l3', 'latin3');
  add('ISO-8859-4',
    'csisolatin4', 'iso-8859-4', 'iso-ir-110', 'iso8859-4', 'iso88594',
    'iso_8859-4', 'iso_8859-4:1988', 'l4', 'latin4');
  add('ISO-8859-5',
    'csisolatincyrillic', 'cyrillic', 'iso-8859-5', 'iso-ir-144', 'iso8859-5', 'iso88595', 'iso_8859-5');
  add('ISO-8859-6',
    'arabic', 'asmo-708', 'csiso88596e', 'csiso88596i', 'csisolatinarabic',
    'ecma-114', 'iso-8859-6', 'iso-8859-6-e', 'iso-8859-6-i', 'iso-ir-127', 'iso8859-6', 'iso88596', 'iso_8859-6');
  add('ISO-8859-7',
    'csisolatingreek', 'ecma-118', 'elot_928', 'greek', 'greek8',
    'iso-8859-7', 'iso-ir-126', 'iso8859-7', 'iso88597', 'iso_8859-7', 'sun_eu_greek');
  add('ISO-8859-8',
    'csiso88598e', 'csisolatinhebrew', 'hebrew', 'iso-8859-8', 'iso-8859-8-e',
    'iso-ir-138', 'iso8859-8', 'iso88598', 'iso_8859-8', 'visual');
  add('ISO-8859-8-I', 'csiso88598i', 'iso-8859-8-i', 'logical');
  add('ISO-8859-10',
    'csisolatin6', 'iso-8859-10', 'iso-ir-157', 'iso8859-10', 'iso885910',
    'iso_8859-10', 'l6', 'latin6');
  add('ISO-8859-13', 'iso-8859-13', 'iso8859-13', 'iso885913');
  add('ISO-8859-14', 'iso-8859-14', 'iso8859-14', 'iso885914');
  add('ISO-8859-15',
    'csisolatin9', 'iso-8859-15', 'iso8859-15', 'iso885915', 'iso_8859-15', 'l9');
  add('ISO-8859-16', 'iso-8859-16');
  add('KOI8-R', 'cskoi8r', 'koi', 'koi8', 'koi8-r', 'koi8_r');
  add('KOI8-U', 'koi8-ru', 'koi8-u');
  add('IBM866', '866', 'cp866', 'csibm866', 'ibm866');
  add('macintosh', 'csmacintosh', 'mac', 'macintosh', 'x-mac-roman');
  add('x-mac-cyrillic', 'x-mac-cyrillic', 'x-mac-ukrainian');
  add('GBK', 'chinese', 'csgb2312', 'csiso58gb231280', 'gb2312', 'gb_2312', 'gb_2312-80', 'gbk', 'iso-ir-58', 'x-gbk');
  add('gb18030', 'gb18030');
  add('Big5', 'big5', 'big5-hkscs', 'cn-big5', 'csbig5', 'x-x-big5');
  add('EUC-JP', 'cseucpkdfmtjapanese', 'euc-jp', 'x-euc-jp');
  add('ISO-2022-JP', 'csiso2022jp', 'iso-2022-jp');
  add('Shift_JIS', 'csshiftjis', 'ms932', 'ms_kanji', 'shift-jis', 'shift_jis', 'sjis', 'windows-31j', 'x-sjis');
  add('EUC-KR', 'cseuckr', 'csksc56011987', 'euc-kr', 'iso-ir-149', 'korean', 'ks_c_5601-1987', 'ks_c_5601-1989', 'ksc5601', 'ksc_5601', 'windows-949');
  add('replacement', 'csiso2022kr', 'hz-gb-2312', 'iso-2022-cn', 'iso-2022-cn-ext', 'iso-2022-kr', 'replacement');
  add('x-user-defined', 'x-user-defined');

  return map;
}

/**
 * Map a label string to its canonical WHATWG encoding name, or null.
 * @param {string | null | undefined} label
 * @returns {string | null}
 */
function labelToName(label) {
  if (label == null) return null;
  const key = String(label).trim().toLowerCase();
  return LABEL_MAP.get(key) || null;
}

module.exports = {
  getBOMEncoding,
  labelToName,
  // html-encoding-sniffer only uses the two above; export stubs for the rest
  // so any future require() of this shim doesn't throw on destructuring.
  TextDecoder: globalThis.TextDecoder,
  TextEncoder: globalThis.TextEncoder,
  TextDecoderStream: globalThis.TextDecoderStream,
  TextEncoderStream: globalThis.TextEncoderStream,
  normalizeEncoding: (label) => label ? String(label).trim().toLowerCase() : null,
  legacyHookDecode: undefined,
  isomorphicDecode: undefined,
  isomorphicEncode: undefined,
};
