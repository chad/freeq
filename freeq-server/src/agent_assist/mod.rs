//! Agent Assistance Interface (MVP).
//!
//! Lets coding agents and operators ask the live server *"why is my
//! client doing X?"* and get a safe, explained answer grounded in
//! actual server state. See `docs` and the brief in the project root
//! for the full design.
//!
//! # MVP scope
//!
//! - `GET /.well-known/agent.json` — discovery
//! - `POST /agent/tools/validate_client_config` — pre-flight check on
//!   a client's IRCv3 capability matrix
//! - `POST /agent/tools/diagnose_message_ordering` — compare a
//!   client's observed display order against the canonical sequence
//! - `POST /agent/tools/diagnose_sync` — report what live state the
//!   server can see for an account, and what it intentionally cannot
//!
//! # Architecture
//!
//! The split between [`tools`] and [`api`] is the point. Tools take
//! typed input + a [`types::Caller`] + state, and produce a
//! [`types::FactBundle`] (deterministic, redacted, side-effect-free).
//! The api layer wraps a bundle in [`types::AssistResponse`].
//!
//! The next PR adds an LLM summarizer. It will sit between the tool
//! and the envelope: take a `FactBundle`, pass *only* `safe_facts` to
//! the LLM, replace `summary` with the model's output, and leave
//! everything else alone. The LLM never sees raw server state, never
//! decides disclosure, and never executes server actions — those are
//! all handled deterministically here.
//!
//! # Privacy + safety
//!
//! Every response goes through [`api::envelope`]'s disclosure filter
//! as a defense-in-depth final pass. Tools also perform their own
//! upfront permission check. Caller-supplied free text is sanitized
//! and quoted before being interpolated into our `safe_facts`, so
//! prompt-injection attempts in `symptom` fields cannot escape the
//! quoting. Sensitive fields (raw tokens, message bodies, full
//! policy expressions) are never recorded by [`recorder`] in the
//! first place.

pub mod api;
pub mod caller;
pub mod llm;
pub mod recorder;
pub mod tools;
pub mod types;

pub use recorder::{DiagnosticEvent, EventKind, record};
pub use types::{AssistResponse, Caller, DisclosureLevel, FactBundle};
