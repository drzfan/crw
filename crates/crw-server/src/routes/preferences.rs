use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

/// `GET /metrics/renderer-preferences` — every host the preference learner is
/// currently tracking, and why.
///
/// The learner promotes a host to Chrome-first after enough LightPanda-specific
/// failures, which silently changes that host's renderer ladder AND disables
/// the lightpanda/chrome hedge for it. That is invisible from the outside: the
/// startup banner prints the CONFIGURED order, while the effective order is
/// per-host and decided at request time. This endpoint is how an operator sees
/// the difference, and it ships alongside the config so the counts can be read
/// without knowing the constants.
///
/// State is per-process and in-memory, so this reflects THIS engine container
/// only — a blue/green pair learns independently.
pub async fn renderer_preferences(State(state): State<AppState>) -> impl IntoResponse {
    let prefs = state.renderer.preferences();
    Json(json!({
        "config": crw_renderer::preference::config(),
        "size": prefs.size(),
        "hosts": prefs.snapshot(),
    }))
}

#[derive(Deserialize)]
pub struct ResetBody {
    /// Host to forget. Omit to clear every tracked host.
    pub host: Option<String>,
}

/// `POST /admin/preferences/reset` — forget one host, or all of them.
///
/// Forgetting a host is safe by construction: the learner only ever REORDERS
/// the ladder, so a cleared host goes back to cheap-first and re-learns from
/// its next failures. Mirrors `/admin/breakers/reset`.
pub async fn reset_preferences(
    State(state): State<AppState>,
    body: Option<Json<ResetBody>>,
) -> impl IntoResponse {
    let prefs = state.renderer.preferences();
    match body.and_then(|Json(b)| b.host) {
        Some(host) => {
            prefs.reset_host(&host).await;
            tracing::warn!(%host, "admin: renderer preference reset for host");
            Json(json!({ "ok": true, "host": host }))
        }
        None => {
            let cleared = prefs.size();
            prefs.reset_all().await;
            tracing::warn!(cleared, "admin: all renderer preferences reset");
            Json(json!({ "ok": true, "hosts_cleared": cleared }))
        }
    }
}
