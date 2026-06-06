# freeq AT Protocol lexicons

Forkable personas and characters as **public, signed records** in their
author's PDS repo. This is what makes fork counts and lineage *trustless*
— verifiable from the firehose without trusting any single server.

## Records

- **`at.freeq.persona`** — a persona's brain: system prompt, TTS voice,
  greeting, the ghostly character it wears (`face`), and `forkedFrom`
  (the parent's `at://` URI). The record's own URI is its identity.
- **`at.freeq.character`** — a ghostly character: `base` archetype +
  the serialized ghostly `CharacterPack` (face + voice DSP), with
  `forkedFrom`. Worn by personas via `persona#face.pack`.

## Why lineage is intrinsic

Each record pins its parent in `forkedFrom`. Lineage is therefore part
of the signed record — a fork can't rewrite its ancestor or strip
attribution, and **the forker is the child record's own repo authority
(DID), not a claim the submitter makes.** Rebuild the whole fork graph
from records alone.

## Aggregation (today vs. eventual)

The server keeps an aggregated fork graph (`forks` table) for cheap
queries:

- `POST /api/v1/personas/record` — ingest a record (`{ uri, record }`);
  if it carries `forkedFrom`, the edge is folded into the graph. This is
  a **push-based stand-in** for a firehose indexer. Idempotent per child.
- `GET /api/v1/forks/{kind}?id=<at-uri>` — direct forks + count.
- `GET /api/v1/lineage/{kind}?id=<at-uri>` — ancestor chain to root.

(`{kind}` namespaces ids: `persona` | `character` | `agent`. Use the
`?id=` query form for `at://` URIs, which contain slashes; the
`/{kind}/{id}` path form is for slash-free ids like names/DIDs.)

## Deferred (needs external pieces)

- **Writing records to a user's PDS** — requires their OAuth session
  (`createRecord` scope). Done client-side; the Persona Studio produces
  the record, the user's PDS signs it.
- **Firehose indexer** — subscribe to the relay, filter `at.freeq.*`
  collections, and call the ingest path automatically instead of by push.

The record types + AT-URI handling + fork-edge derivation live in
`freeq-server/src/records.rs`; the canonical `CharacterPack` schema lives
in the `ghostly` crate.
