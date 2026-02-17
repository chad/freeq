//! Channel-Based Software Factory.
//!
//! A persistent channel where coordinated AI agents act as a full
//! software development organization. Humans observe and guide.
//!
//! Agent roles:
//! - Product Lead: clarifies requirements, maintains spec
//! - Architect: proposes design, stack, structure
//! - Builder: writes code via tool use
//! - Reviewer: critiques code quality and spec alignment
//! - QA: generates and runs tests
//! - Deploy: deploys to staging and posts preview URL

mod orchestrator;

pub use orchestrator::{Factory, FactoryConfig};
