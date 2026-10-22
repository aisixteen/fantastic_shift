//! The app's credentials. The key is read from the credential store at each call.
//! A JWT and an installation token are minted per call, live only in that call's
//! locals, and are never cached, returned, stored or logged.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fantastic_kernel::{AgentId, Item, Kernel, Status};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::Method;
use serde_json::{json, Value};

use crate::api::Api;
use crate::Cfg;

/// The app id and private key read from the store. No Debug, Display or Serialize.
pub(crate) struct AppKey {
    pub app_id: String,
    pub pem: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Claims {
    pub iat: u64,
    pub exp: u64,
    pub iss: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cred_error(cfg: &Cfg, what: &str) -> Value {
    json!({"error": format!(
        "team_github: credential `{}` in `{}`: {what}",
        cfg.app_cred.1, cfg.app_cred.0
    )})
}

/// Read `app_id` and `private_key` from the credential store, sent as this agent.
/// Errors name the ref and store, never a value.
pub(crate) async fn read_app(
    id: &AgentId,
    kernel: &Arc<Kernel>,
    cfg: &Cfg,
) -> Result<AppKey, Value> {
    let (store, r) = cfg.app_cred.clone();
    let _ = id;
    let outs = kernel
        .send(vec![Item::new(
            store.as_str(),
            json!({"type": "get", "ref": r, "host": cfg.api_url}),
        )])
        .await;
    let Some(out) = outs.into_iter().next() else {
        return Err(cred_error(cfg, "no reply from the store"));
    };
    if !matches!(out.status, Status::Completed) || !out.result.is_object() {
        return Err(cred_error(cfg, "the store refused the read"));
    }
    if out.result.get("error").is_some_and(|v| match v {
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    }) {
        return Err(cred_error(cfg, "the store refused the read"));
    }
    let fields = out.result.get("fields").cloned().unwrap_or(Value::Null);
    let app_id = match fields.get("app_id") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) if n.as_u64().is_some_and(|v| v > 0) => n.to_string(),
        _ => return Err(cred_error(cfg, "has no `app_id` field")),
    };
    let pem = match fields.get("private_key") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => return Err(cred_error(cfg, "has no `private_key` field")),
    };
    Ok(AppKey { app_id, pem })
}

/// Sign an RS256 JWT for the app: issued 60 s back, valid 10 minutes at most.
pub(crate) fn sign_jwt(app_id: &str, pem: &str, now: u64) -> Result<String, String> {
    let claims = Claims {
        iat: now.saturating_sub(60),
        exp: now + 540,
        iss: app_id.to_string(),
    };
    let key = EncodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|_| "private_key is not an RSA private key in PEM form".to_string())?;
    jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
        .map_err(|_| "could not sign the JWT".to_string())
}

/// A fresh JWT for this call. The key is dropped when this returns.
pub(crate) async fn app_jwt(
    id: &AgentId,
    kernel: &Arc<Kernel>,
    cfg: &Cfg,
) -> Result<String, Value> {
    let key = read_app(id, kernel, cfg).await?;
    sign_jwt(&key.app_id, &key.pem, now_secs()).map_err(|e| cred_error(cfg, &e))
}

/// An installation token scoped to the one repo, minted for this call only.
pub(crate) async fn token_for(
    id: &AgentId,
    kernel: &Arc<Kernel>,
    cfg: &Cfg,
    api: &Api,
    verb: &str,
    repo: &str,
) -> Result<String, Value> {
    let jwt = app_jwt(id, kernel, cfg).await?;
    let jw = jwt.as_str();
    let inst = api
        .call(
            verb,
            Method::GET,
            &format!("/repos/{repo}/installation"),
            &[],
            jw,
            None,
        )
        .await
        .map_err(|(_, v)| v)?;
    let install = inst.get("id").and_then(|v| v.as_u64()).ok_or_else(
        || json!({"error": format!("team_github {verb}: no installation id for {repo}")}),
    )?;
    let name = repo.split('/').nth(1).unwrap_or("");
    let minted = api
        .call(
            verb,
            Method::POST,
            &format!("/app/installations/{install}/access_tokens"),
            &[],
            jw,
            Some(&json!({"repositories": [name]})),
        )
        .await
        .map_err(|(_, v)| v)?;
    minted
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .ok_or_else(|| json!({"error": format!("team_github {verb}: GitHub minted no token")}))
}
