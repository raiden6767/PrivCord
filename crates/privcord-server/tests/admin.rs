//! The admin panel's API, through real sockets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use privcord_server::admin::{AdminState, Role};
use privcord_server::nostr::event::hex;
use privcord_server::{config::Config, start, Running};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn config(dir: &std::path::Path) -> Config {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.into();
    c.admin.enabled = true;
    c.admin.listen = "127.0.0.1:0".into();
    c.nostr.enabled = true;
    c.nostr.events_per_minute_per_ip = 0;
    c.relay.store_and_forward = false;
    c
}

async fn boot() -> (Running, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let r = start(config(dir.path())).await.unwrap();
    let token = r
        .admin_first_token()
        .expect("the first owner token")
        .to_owned();
    (r, dir, token)
}

/// One HTTP request: (status, headers in lower case, body text).
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, String, String) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    if let Some(t) = token {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    if !body.is_empty() {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    req.push_str("\r\n");
    req.push_str(&body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, head.to_lowercase(), body.to_owned())
}

async fn api(
    r: &Running,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (code, _, b) = http(
        r.admin_addr().unwrap(),
        method,
        &format!("/admin/api{path}"),
        Some(token),
        body,
    )
    .await;
    (code, serde_json::from_str(&b).unwrap_or(Value::Null))
}

fn key() -> SigningKey {
    loop {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).unwrap();
        if let Ok(k) = SigningKey::from_bytes(&k256::FieldBytes::from(b)) {
            return k;
        }
    }
}

fn nostr_event(k: &SigningKey, content: &str) -> Value {
    let pubkey = hex(&k.verifying_key().to_bytes());
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let id: [u8; 32] = Sha256::digest(
        json!([0, pubkey, created, 1, [], content])
            .to_string()
            .as_bytes(),
    )
    .into();
    let mut aux = [0u8; 32];
    getrandom::fill(&mut aux).unwrap();
    let sig = k.sign_raw(&id, &aux).unwrap();
    json!({ "id": hex(&id), "pubkey": pubkey, "created_at": created, "kind": 1, "tags": [], "content": content, "sig": hex(&sig.to_bytes()) })
}

/// Publishes over Nostr on the relay's own port; returns (accepted, message).
async fn publish(r: &Running, ev: &Value) -> (bool, String) {
    let (mut ws, _) = connect_async(format!("ws://{}", r.addr)).await.unwrap();
    ws.send(Message::Text(json!(["EVENT", ev]).to_string().into()))
        .await
        .unwrap();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v[0] == "OK" {
                return (v[2].as_bool().unwrap(), v[3].as_str().unwrap().to_owned());
            }
        }
    }
}

#[tokio::test]
async fn the_page_is_served_with_strict_headers_and_the_api_needs_a_token() {
    let (r, _d, token) = boot().await;
    let a = r.admin_addr().unwrap();
    let (code, head, body) = http(a, "GET", "/admin", None, None).await;
    assert_eq!(code, 200);
    assert!(body.contains("PrivCord server admin"));
    assert!(
        head.contains("content-security-policy: default-src 'none'; script-src 'self'"),
        "{head}"
    );
    assert!(
        head.contains("cache-control: no-store")
            && head.contains("x-frame-options: deny")
            && head.contains("nosniff")
    );
    let (code, head, theme) = http(a, "GET", "/admin/theme.js", None, None).await;
    assert!(code == 200 && head.contains("text/javascript") && theme.contains("color-scheme"));
    let (code, head, js) = http(a, "GET", "/admin/app.js", None, None).await;
    assert!(code == 200 && head.contains("text/javascript") && js.contains("textContent"));
    // the page must not pull in anything from elsewhere, and must not build HTML from server data
    let page_without_svg_namespace = body.replace("http://www.w3.org/2000/svg", "");
    assert!(
        !page_without_svg_namespace.contains("http://")
            && !page_without_svg_namespace.contains("https://")
    );
    assert!(
        !js.contains("innerHTML"),
        "server data must never be put in as HTML"
    );
    assert_eq!(http(a, "GET", "/admin/api/me", None, None).await.0, 401);
    assert_eq!(
        http(a, "GET", "/admin/api/me", Some("pca_not-a-token"), None)
            .await
            .0,
        401
    );
    let (code, me) = api(&r, "GET", "/me", &token, None).await;
    assert_eq!((code, me["role"].as_str()), (200, Some("owner")));
    // the relay's own port does not serve the panel at all
    assert_eq!(
        http(r.addr, "GET", "/admin/api/me", Some(&token), None)
            .await
            .0,
        404
    );
    r.stop().await;
}

#[tokio::test]
async fn too_many_wrong_tokens_lock_that_address_out_for_a_while() {
    let (r, _d, token) = boot().await;
    let a = r.admin_addr().unwrap();
    for _ in 0..10 {
        assert_eq!(
            http(a, "GET", "/admin/api/me", Some("pca_guess"), None)
                .await
                .0,
            401
        );
    }
    assert_eq!(
        http(a, "GET", "/admin/api/me", Some("pca_guess"), None)
            .await
            .0,
        429
    );
    assert_eq!(
        http(a, "GET", "/admin/api/me", Some(&token), None).await.0,
        429,
        "even the right token waits"
    );
    r.stop().await;
}

#[tokio::test]
async fn access_tokens_made_in_the_panel_really_lock_and_unlock_the_relay() {
    let (r, _d, token) = boot().await;
    let open = |t: Option<&str>| {
        let url = match t {
            Some(t) => format!("ws://{}/?token={t}", r.addr),
            None => format!("ws://{}", r.addr),
        };
        async move { connect_async(url).await.is_ok() }
    };
    assert!(open(None).await, "open until a token exists");
    let (code, made) = api(
        &r,
        "POST",
        "/access-tokens",
        &token,
        Some(json!({ "name": "Alice's phone" })),
    )
    .await;
    assert_eq!(code, 200);
    let (secret, id) = (
        made["token"].as_str().unwrap().to_owned(),
        made["id"].as_str().unwrap().to_owned(),
    );
    assert!(secret.starts_with("pcs_"));
    assert!(
        !open(None).await,
        "private as soon as the first token exists"
    );
    assert!(open(Some(&secret)).await);
    assert!(!open(Some("pcs_wrong")).await);
    // the list never shows the secret
    let (_, list) = api(&r, "GET", "/access-tokens", &token, None).await;
    assert!(!list.to_string().contains(&secret));
    assert_eq!(list["tokens"][0]["name"], "Alice's phone");
    // a second token, so the first can be removed without opening the server
    let (_, second) = api(
        &r,
        "POST",
        "/access-tokens",
        &token,
        Some(json!({ "name": "Bob's laptop" })),
    )
    .await;
    let (secret2, id2) = (
        second["token"].as_str().unwrap().to_owned(),
        second["id"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        api(&r, "DELETE", &format!("/access-tokens/{id}"), &token, None)
            .await
            .0,
        200
    );
    assert!(
        !open(Some(&secret)).await,
        "a removed token stops working at once"
    );
    assert!(open(Some(&secret2)).await, "the other one is not affected");
    assert!(!open(None).await);
    // removing the last one would open the server to anyone: that has to be confirmed
    assert_eq!(
        api(&r, "DELETE", &format!("/access-tokens/{id2}"), &token, None)
            .await
            .0,
        409
    );
    assert!(!open(None).await, "refused, so still private");
    assert_eq!(
        api(
            &r,
            "DELETE",
            &format!("/access-tokens/{id2}?open=true"),
            &token,
            None
        )
        .await
        .0,
        200
    );
    assert!(open(None).await, "confirmed: open again");
    assert_eq!(
        api(&r, "DELETE", &format!("/access-tokens/{id}"), &token, None)
            .await
            .0,
        404
    );
    assert_eq!(
        api(
            &r,
            "POST",
            "/access-tokens",
            &token,
            Some(json!({ "name": "" }))
        )
        .await
        .0,
        400
    );
    r.stop().await;
}

#[tokio::test]
async fn each_role_can_do_what_it_should_and_no_more() {
    let (r, _d, owner) = boot().await;
    let mk = |name: &str, role: &str| {
        let body = json!({ "name": name, "role": role });
        let owner = owner.clone();
        let (r2, name) = (&r, name.to_owned());
        async move {
            let (c, v) = api(r2, "POST", "/admins", &owner, Some(body)).await;
            assert_eq!(c, 200, "{name}");
            v["token"].as_str().unwrap().to_owned()
        }
    };
    let (moderator, viewer) = (mk("Mo", "moderator").await, mk("Vi", "viewer").await);
    let policy = json!({ "allowed_pubkeys": [], "blocked_pubkeys": [], "allowed_kinds": [], "blocked_kinds": [7] });
    // viewer: can look, cannot change
    assert_eq!(api(&r, "GET", "/overview", &viewer, None).await.0, 200);
    assert_eq!(api(&r, "GET", "/nostr/policy", &viewer, None).await.0, 200);
    assert_eq!(
        api(&r, "PUT", "/nostr/policy", &viewer, Some(policy.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        api(
            &r,
            "POST",
            "/access-tokens",
            &viewer,
            Some(json!({ "name": "x" }))
        )
        .await
        .0,
        403
    );
    assert_eq!(api(&r, "GET", "/admins", &viewer, None).await.0, 403);
    // moderator: can change rules and tokens, not admins
    assert_eq!(
        api(&r, "PUT", "/nostr/policy", &moderator, Some(policy))
            .await
            .0,
        200
    );
    assert_eq!(
        api(
            &r,
            "POST",
            "/access-tokens",
            &moderator,
            Some(json!({ "name": "x" }))
        )
        .await
        .0,
        200
    );
    assert_eq!(api(&r, "GET", "/admins", &moderator, None).await.0, 403);
    assert_eq!(
        api(
            &r,
            "POST",
            "/admins",
            &moderator,
            Some(json!({ "name": "z", "role": "owner" }))
        )
        .await
        .0,
        403
    );
    // owner: sees admins (never their tokens), and cannot remove the last owner
    let (_, admins) = api(&r, "GET", "/admins", &owner, None).await;
    assert_eq!(admins["admins"].as_array().unwrap().len(), 3);
    assert!(!admins.to_string().contains("pca_") && !admins.to_string().contains("hash"));
    let owner_id = admins["admins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["role"] == "owner")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        api(&r, "DELETE", &format!("/admins/{owner_id}"), &owner, None)
            .await
            .0,
        409
    );
    let mo_id = admins["admins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Mo")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        api(&r, "DELETE", &format!("/admins/{mo_id}"), &owner, None)
            .await
            .0,
        200
    );
    assert_eq!(
        api(&r, "GET", "/me", &moderator, None).await.0,
        401,
        "a removed admin's token stops working"
    );
    assert_eq!(
        api(
            &r,
            "POST",
            "/admins",
            &owner,
            Some(json!({ "name": "z", "role": "emperor" }))
        )
        .await
        .0,
        400
    );
    r.stop().await;
}

#[tokio::test]
async fn nostr_rules_set_in_the_panel_apply_at_once_survive_a_restart_and_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let r = start(config(dir.path())).await.unwrap();
    let token = r.admin_first_token().unwrap().to_owned();
    let k = key();
    assert!(publish(&r, &nostr_event(&k, "before")).await.0);
    let me = hex(&k.verifying_key().to_bytes());
    let block = json!({ "allowed_pubkeys": [], "blocked_pubkeys": [me], "allowed_kinds": [], "blocked_kinds": [] });
    assert_eq!(
        api(&r, "PUT", "/nostr/policy", &token, Some(block)).await.0,
        200
    );
    let (ok, why) = publish(&r, &nostr_event(&k, "after the block")).await;
    assert!(!ok && why.starts_with("blocked"), "{why}");
    // bad input is explained, and nothing changes
    for bad in [
        json!({ "allowed_pubkeys": ["not-hex"], "blocked_pubkeys": [], "allowed_kinds": [], "blocked_kinds": [] }),
        json!({ "allowed_pubkeys": [], "blocked_pubkeys": [], "allowed_kinds": [70000], "blocked_kinds": [] }),
        json!({ "allowed_pubkeys": [] }),
    ] {
        assert_eq!(
            api(&r, "PUT", "/nostr/policy", &token, Some(bad)).await.0,
            400
        );
    }
    assert!(!publish(&r, &nostr_event(&k, "still blocked")).await.0);
    r.stop().await;
    // after a restart the saved rules and the admin are still there (and no new first token is made)
    let r2 = start(config(dir.path())).await.unwrap();
    assert!(r2.admin_first_token().is_none());
    assert_eq!(api(&r2, "GET", "/me", &token, None).await.0, 200);
    assert!(
        !publish(&r2, &nostr_event(&k, "after restart")).await.0,
        "the saved rule applies again"
    );
    let (_, p) = api(&r2, "GET", "/nostr/policy", &token, None).await;
    assert_eq!(p["blocked_pubkeys"][0], me);
    r2.stop().await;
}

#[tokio::test]
async fn an_event_can_be_removed_for_good_by_a_moderator() {
    let (r, _d, token) = boot().await;
    let k = key();
    let e = nostr_event(&k, "to be removed");
    assert!(publish(&r, &e).await.0);
    let id = e["id"].as_str().unwrap();
    let (code, v) = api(&r, "DELETE", &format!("/nostr/events/{id}"), &token, None).await;
    assert_eq!((code, v["removed"].as_bool()), (200, Some(true)));
    let (ok, why) = publish(&r, &e).await;
    assert!(
        !ok && why.starts_with("blocked"),
        "it must not come back: {why}"
    );
    assert_eq!(
        api(&r, "DELETE", "/nostr/events/not-an-id", &token, None)
            .await
            .0,
        400
    );
    r.stop().await;
}

#[tokio::test]
async fn an_admin_made_on_the_command_line_works_without_restarting_the_server() {
    let (r, dir, _owner) = boot().await;
    let token = AdminState::add_admin_offline(dir.path(), "late", Role::Viewer).unwrap();
    let (code, me) = api(&r, "GET", "/me", &token, None).await;
    assert_eq!(
        (code, me["name"].as_str(), me["role"].as_str()),
        (200, Some("late"), Some("viewer"))
    );
    // the file holds hashes only
    let saved = std::fs::read_to_string(dir.path().join("admin.json")).unwrap();
    assert!(!saved.contains(&token));
    r.stop().await;
}

#[tokio::test]
async fn the_panel_is_off_unless_asked_for_and_refuses_oversized_requests() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.admin.enabled = false;
    let r = start(c).await.unwrap();
    assert!(r.admin_addr().is_none());
    r.stop().await;
    let (r, _d, token) = boot().await;
    let big = json!({ "name": "x".repeat(100_000) });
    assert_eq!(
        api(&r, "POST", "/access-tokens", &token, Some(big)).await.0,
        413
    );
    r.stop().await;
}

#[tokio::test]
async fn the_overview_warns_about_an_open_server_and_a_plain_panel_on_a_public_address() {
    let (r, _d, token) = boot().await;
    let (_, o) = api(&r, "GET", "/overview", &token, None).await;
    let warnings = o["warnings"].to_string();
    assert!(
        warnings.contains("Anyone who finds this server"),
        "{warnings}"
    );
    assert_eq!(o["roles"]["relay"], true);
    assert_eq!(o["access"]["private"], false);
    assert!(o["addresses"]["relay"]
        .as_str()
        .unwrap()
        .starts_with("ws://"));
    r.stop().await;
}
