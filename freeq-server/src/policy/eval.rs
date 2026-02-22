//! Requirement DSL evaluator.
//!
//! Evaluates requirements against a user's provided evidence.
//! Deterministic, side-effect free, fail-closed on unknown types.
//!
//! Constraints enforced:
//! - Max depth: 8
//! - Max nodes: 64

use super::types::Requirement;
use std::collections::HashSet;

/// Evidence provided by a user during a join or role request.
#[derive(Debug, Clone)]
pub struct UserEvidence {
    /// Hashes of rules documents the user has accepted.
    pub accepted_hashes: HashSet<String>,
    /// Credentials the user presents: (credential_type, issuer).
    pub credentials: Vec<Credential>,
    /// Proofs the user can provide: proof_type identifiers.
    pub proofs: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct Credential {
    pub credential_type: String,
    pub issuer: String,
}

/// Result of evaluating a requirement.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalResult {
    /// Requirement satisfied.
    Satisfied,
    /// Requirement not satisfied, with reason.
    Failed(String),
    /// Evaluation error (depth/node limit, malformed).
    Error(String),
}

impl EvalResult {
    pub fn is_satisfied(&self) -> bool {
        matches!(self, EvalResult::Satisfied)
    }
}

const MAX_DEPTH: u32 = 8;
const MAX_NODES: u32 = 64;

/// Evaluate a requirement tree against user evidence.
pub fn evaluate(requirement: &Requirement, evidence: &UserEvidence) -> EvalResult {
    let mut node_count = 0;
    eval_inner(requirement, evidence, 0, &mut node_count)
}

/// Validate a requirement tree structure (depth/node limits).
pub fn validate_structure(requirement: &Requirement) -> Result<(), String> {
    let mut node_count = 0;
    validate_inner(requirement, 0, &mut node_count)
}

fn validate_inner(req: &Requirement, depth: u32, node_count: &mut u32) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("Requirement tree exceeds max depth of {MAX_DEPTH}"));
    }
    *node_count += 1;
    if *node_count > MAX_NODES {
        return Err(format!("Requirement tree exceeds max node count of {MAX_NODES}"));
    }

    match req {
        Requirement::Accept { .. } | Requirement::Present { .. } | Requirement::Prove { .. } => {
            Ok(())
        }
        Requirement::All { requirements } | Requirement::Any { requirements } => {
            if requirements.is_empty() {
                return Err("ALL/ANY must have at least one sub-requirement".to_string());
            }
            for r in requirements {
                validate_inner(r, depth + 1, node_count)?;
            }
            Ok(())
        }
        Requirement::Not { requirement } => validate_inner(requirement, depth + 1, node_count),
    }
}

fn eval_inner(
    req: &Requirement,
    evidence: &UserEvidence,
    depth: u32,
    node_count: &mut u32,
) -> EvalResult {
    if depth > MAX_DEPTH {
        return EvalResult::Error(format!("Max depth {MAX_DEPTH} exceeded"));
    }
    *node_count += 1;
    if *node_count > MAX_NODES {
        return EvalResult::Error(format!("Max node count {MAX_NODES} exceeded"));
    }

    match req {
        Requirement::Accept { hash } => {
            if evidence.accepted_hashes.contains(hash) {
                EvalResult::Satisfied
            } else {
                EvalResult::Failed(format!("User has not accepted rules hash: {hash}"))
            }
        }

        Requirement::Present {
            credential_type,
            issuer,
        } => {
            let found = evidence.credentials.iter().any(|c| {
                c.credential_type == *credential_type
                    && issuer
                        .as_ref()
                        .map_or(true, |req_issuer| c.issuer == *req_issuer)
            });
            if found {
                EvalResult::Satisfied
            } else {
                let msg = match issuer {
                    Some(iss) => format!("Missing credential: {credential_type} from {iss}"),
                    None => format!("Missing credential: {credential_type}"),
                };
                EvalResult::Failed(msg)
            }
        }

        Requirement::Prove { proof_type } => {
            if evidence.proofs.contains(proof_type) {
                EvalResult::Satisfied
            } else {
                EvalResult::Failed(format!("Missing proof: {proof_type}"))
            }
        }

        Requirement::All { requirements } => {
            for r in requirements {
                let result = eval_inner(r, evidence, depth + 1, node_count);
                if !result.is_satisfied() {
                    return result;
                }
            }
            EvalResult::Satisfied
        }

        Requirement::Any { requirements } => {
            let mut last_failure = String::new();
            for r in requirements {
                let result = eval_inner(r, evidence, depth + 1, node_count);
                match result {
                    EvalResult::Satisfied => return EvalResult::Satisfied,
                    EvalResult::Failed(msg) => last_failure = msg,
                    EvalResult::Error(e) => return EvalResult::Error(e),
                }
            }
            EvalResult::Failed(format!("No alternative satisfied: {last_failure}"))
        }

        Requirement::Not { requirement } => {
            let result = eval_inner(requirement, evidence, depth + 1, node_count);
            match result {
                EvalResult::Satisfied => {
                    EvalResult::Failed("NOT condition: inner requirement was satisfied".to_string())
                }
                EvalResult::Failed(_) => EvalResult::Satisfied,
                EvalResult::Error(e) => EvalResult::Error(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_evidence() -> UserEvidence {
        UserEvidence {
            accepted_hashes: HashSet::new(),
            credentials: vec![],
            proofs: HashSet::new(),
        }
    }

    #[test]
    fn test_accept_satisfied() {
        let req = Requirement::Accept {
            hash: "abc123".into(),
        };
        let mut ev = empty_evidence();
        ev.accepted_hashes.insert("abc123".into());
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_accept_failed() {
        let req = Requirement::Accept {
            hash: "abc123".into(),
        };
        let ev = empty_evidence();
        assert!(!evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_present_with_issuer() {
        let req = Requirement::Present {
            credential_type: "github_membership".into(),
            issuer: Some("github".into()),
        };
        let mut ev = empty_evidence();
        ev.credentials.push(Credential {
            credential_type: "github_membership".into(),
            issuer: "github".into(),
        });
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_present_wrong_issuer() {
        let req = Requirement::Present {
            credential_type: "github_membership".into(),
            issuer: Some("github".into()),
        };
        let mut ev = empty_evidence();
        ev.credentials.push(Credential {
            credential_type: "github_membership".into(),
            issuer: "gitlab".into(),
        });
        assert!(!evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_present_any_issuer() {
        let req = Requirement::Present {
            credential_type: "email".into(),
            issuer: None,
        };
        let mut ev = empty_evidence();
        ev.credentials.push(Credential {
            credential_type: "email".into(),
            issuer: "google".into(),
        });
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_prove() {
        let req = Requirement::Prove {
            proof_type: "github_repo_write_access".into(),
        };
        let mut ev = empty_evidence();
        ev.proofs.insert("github_repo_write_access".into());
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_all_both_satisfied() {
        let req = Requirement::All {
            requirements: vec![
                Requirement::Accept {
                    hash: "rules".into(),
                },
                Requirement::Prove {
                    proof_type: "kyc".into(),
                },
            ],
        };
        let mut ev = empty_evidence();
        ev.accepted_hashes.insert("rules".into());
        ev.proofs.insert("kyc".into());
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_all_one_fails() {
        let req = Requirement::All {
            requirements: vec![
                Requirement::Accept {
                    hash: "rules".into(),
                },
                Requirement::Prove {
                    proof_type: "kyc".into(),
                },
            ],
        };
        let mut ev = empty_evidence();
        ev.accepted_hashes.insert("rules".into());
        // No kyc proof
        assert!(!evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_any_first_satisfied() {
        let req = Requirement::Any {
            requirements: vec![
                Requirement::Accept {
                    hash: "a".into(),
                },
                Requirement::Accept {
                    hash: "b".into(),
                },
            ],
        };
        let mut ev = empty_evidence();
        ev.accepted_hashes.insert("a".into());
        assert!(evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_any_none_satisfied() {
        let req = Requirement::Any {
            requirements: vec![
                Requirement::Accept {
                    hash: "a".into(),
                },
                Requirement::Accept {
                    hash: "b".into(),
                },
            ],
        };
        let ev = empty_evidence();
        assert!(!evaluate(&req, &ev).is_satisfied());
    }

    #[test]
    fn test_not() {
        let req = Requirement::Not {
            requirement: Box::new(Requirement::Accept {
                hash: "banned".into(),
            }),
        };
        let ev = empty_evidence();
        // User has NOT accepted "banned" → NOT(failed) = satisfied
        assert!(evaluate(&req, &ev).is_satisfied());

        let mut ev2 = empty_evidence();
        ev2.accepted_hashes.insert("banned".into());
        // User HAS accepted "banned" → NOT(satisfied) = failed
        assert!(!evaluate(&req, &ev2).is_satisfied());
    }

    #[test]
    fn test_github_use_case() {
        // From spec: anyone joins by accepting rules, ops need github membership
        let join_req = Requirement::Accept {
            hash: "channel_rules_v1".into(),
        };
        let op_req = Requirement::All {
            requirements: vec![
                Requirement::Accept {
                    hash: "channel_rules_v1".into(),
                },
                Requirement::Present {
                    credential_type: "github_membership".into(),
                    issuer: Some("github".into()),
                },
            ],
        };

        // Regular user: accepted rules, no github
        let mut regular = empty_evidence();
        regular.accepted_hashes.insert("channel_rules_v1".into());
        assert!(evaluate(&join_req, &regular).is_satisfied());
        assert!(!evaluate(&op_req, &regular).is_satisfied());

        // GitHub committer: accepted rules + github cred
        let mut committer = regular.clone();
        committer.credentials.push(Credential {
            credential_type: "github_membership".into(),
            issuer: "github".into(),
        });
        assert!(evaluate(&join_req, &committer).is_satisfied());
        assert!(evaluate(&op_req, &committer).is_satisfied());
    }

    #[test]
    fn test_validate_depth_limit() {
        // Build a deeply nested NOT chain
        let mut req = Requirement::Accept {
            hash: "x".into(),
        };
        for _ in 0..10 {
            req = Requirement::Not {
                requirement: Box::new(req),
            };
        }
        assert!(validate_structure(&req).is_err());
    }

    #[test]
    fn test_validate_node_limit() {
        // Build a wide ALL with 65 children
        let children: Vec<_> = (0..65)
            .map(|i| Requirement::Accept {
                hash: format!("h{i}"),
            })
            .collect();
        let req = Requirement::All {
            requirements: children,
        };
        assert!(validate_structure(&req).is_err());
    }

    #[test]
    fn test_validate_empty_all() {
        let req = Requirement::All {
            requirements: vec![],
        };
        assert!(validate_structure(&req).is_err());
    }
}
