//! # AI Runtime Intelligence (Phase 8)
//!
//! Read-only observability for **already-discovered** local AI services:
//! Ollama, llama.cpp, ComfyUI, Gradio, Open WebUI, and generic AI web
//! runtimes. Detection itself stays in Phase 3 (`intelligence::rules`) —
//! this module only *verifies and enriches* those identities.
//!
//! ## Structure
//!
//! - [`domain`] — pure domain model, endpoint trust policy, HTTP bounds.
//! - `adapters` (next) — per-runtime inspection logic with fixture tests.
//! - `registry` (next) — snapshot cache, probe cadence, bounded concurrency.
//!
//! ## Hard boundaries
//!
//! - **Trust**: probes run only for runtime identities resolved from the
//!   Phase 3 classifier against the *current* discovery snapshot; the
//!   frontend passes `runtimeId`s only, never URLs (spec §58).
//! - **Endpoints**: loopback-only policy enforced in [`domain`]
//!   (`http://localhost`, `http://127.0.0.1`, `http://[::1]`); redirects are
//!   never followed; credentials in URLs are rejected before any request.
//! - **No inference**: only read-only inspection endpoints are ever called
//!   (spec §7, §45) — no chat/generate/completion/embeddings/workflows.
//! - **No model management**: no pull/delete/load/unload/copy (spec §44).
//! - **Privacy**: model names + runtime metadata only; never prompts,
//!   conversations, cookies, tokens, or accounts (spec §42). Nothing is
//!   transmitted off the machine.

pub(crate) mod adapters;
pub(crate) mod domain;
pub(crate) mod probe;
pub(crate) mod registry;

#[cfg(test)]
mod tests {
    use super::domain::*;

    #[test]
    fn endpoint_policy_smoke() {
        assert_eq!(validate_endpoint("http://127.0.0.1:11434"), Ok(()));
        assert!(validate_endpoint("http://192.168.1.2:80").is_err());
    }
}
