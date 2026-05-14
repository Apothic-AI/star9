#![cfg(all(feature = "native-http", not(target_arch = "wasm32")))]

use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use http::Method;
use wanix_fs::{
    AwsSigV4Signer, HttpRequest, HttpTransport, NativeHttpTransport, Object, ObjectStore, Result,
    S3ObjectStore,
};

fn opt_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn live_enabled(name: &str) -> bool {
    matches!(opt_env(name).as_deref(), Some("1" | "true" | "yes" | "on"))
}

#[test]
fn live_http_backend_exercises_real_server_edges_when_configured() -> Result<()> {
    if !live_enabled("WANIX_LIVE_HTTP") {
        eprintln!("skipping live HTTP backend test; set WANIX_LIVE_HTTP=1");
        return Ok(());
    }
    let Some(base_url) = opt_env("WANIX_LIVE_HTTP_BASE_URL") else {
        eprintln!("skipping live HTTP backend test; WANIX_LIVE_HTTP_BASE_URL is unset");
        return Ok(());
    };
    let transport = NativeHttpTransport::new();

    let mut headers = BTreeMap::new();
    if let Some(auth) = opt_env("WANIX_LIVE_HTTP_AUTH") {
        headers.insert("Authorization".to_string(), auth);
    }
    let get = transport.request(HttpRequest {
        method: Method::GET,
        url: base_url.clone(),
        headers: headers.clone(),
        body: Vec::new(),
    })?;
    assert!(
        (200..=299).contains(&get.status),
        "GET {base_url} returned {}",
        get.status
    );

    let head = transport.request(HttpRequest {
        method: Method::HEAD,
        url: base_url.clone(),
        headers: headers.clone(),
        body: Vec::new(),
    })?;
    assert!(
        (200..=399).contains(&head.status),
        "HEAD {base_url} returned {}",
        head.status
    );

    if let Some(etag) = header(&get.headers, "etag").or_else(|| header(&head.headers, "etag")) {
        let mut conditional_headers = headers.clone();
        conditional_headers.insert("If-None-Match".to_string(), etag.to_string());
        let conditional = transport.request(HttpRequest {
            method: Method::GET,
            url: base_url.clone(),
            headers: conditional_headers,
            body: Vec::new(),
        })?;
        assert!(
            matches!(conditional.status, 200 | 304),
            "conditional GET {base_url} returned {}",
            conditional.status
        );
    }

    if let Some(range_url) = opt_env("WANIX_LIVE_HTTP_RANGE_URL") {
        let mut range_headers = headers.clone();
        range_headers.insert("Range".to_string(), "bytes=0-3".to_string());
        let range = transport.request(HttpRequest {
            method: Method::GET,
            url: range_url.clone(),
            headers: range_headers,
            body: Vec::new(),
        })?;
        assert!(
            matches!(range.status, 200 | 206 | 416),
            "range GET {range_url} returned {}",
            range.status
        );
    }

    if let Some(redirect_url) = opt_env("WANIX_LIVE_HTTP_REDIRECT_URL") {
        let redirect = transport.request(HttpRequest {
            method: Method::GET,
            url: redirect_url.clone(),
            headers: headers.clone(),
            body: Vec::new(),
        })?;
        assert!(
            (200..=399).contains(&redirect.status),
            "redirect GET {redirect_url} returned {}",
            redirect.status
        );
    }

    if let Some(auth_failure_url) = opt_env("WANIX_LIVE_HTTP_AUTH_FAILURE_URL") {
        let auth_failure = transport.request(HttpRequest {
            method: Method::GET,
            url: auth_failure_url.clone(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        })?;
        assert!(
            matches!(auth_failure.status, 401 | 403),
            "auth failure GET {auth_failure_url} returned {}",
            auth_failure.status
        );
    }

    Ok(())
}

#[test]
fn live_s3_r2_bucket_exercises_object_store_edges_when_configured() -> Result<()> {
    if !live_enabled("WANIX_LIVE_S3") {
        eprintln!("skipping live S3/R2 bucket test; set WANIX_LIVE_S3=1");
        return Ok(());
    }
    let Some(endpoint) = opt_env("WANIX_S3_ENDPOINT") else {
        eprintln!("skipping live S3/R2 bucket test; WANIX_S3_ENDPOINT is unset");
        return Ok(());
    };
    let Some(bucket) = opt_env("WANIX_S3_BUCKET") else {
        eprintln!("skipping live S3/R2 bucket test; WANIX_S3_BUCKET is unset");
        return Ok(());
    };
    let Some(access_key_id) = opt_env("WANIX_S3_ACCESS_KEY_ID") else {
        eprintln!("skipping live S3/R2 bucket test; WANIX_S3_ACCESS_KEY_ID is unset");
        return Ok(());
    };
    let Some(secret_access_key) = opt_env("WANIX_S3_SECRET_ACCESS_KEY") else {
        eprintln!("skipping live S3/R2 bucket test; WANIX_S3_SECRET_ACCESS_KEY is unset");
        return Ok(());
    };
    let region = opt_env("WANIX_S3_REGION").unwrap_or_else(|| "auto".to_string());
    let service = opt_env("WANIX_S3_SERVICE").unwrap_or_else(|| "s3".to_string());
    let prefix = opt_env("WANIX_S3_PREFIX").unwrap_or_else(default_live_prefix);
    let signer = AwsSigV4Signer::with_service(access_key_id, secret_access_key, region, service);
    let store = S3ObjectStore::with_signer(
        endpoint,
        bucket,
        Some(prefix.clone()),
        Arc::new(NativeHttpTransport::new()),
        Arc::new(signer),
    );

    let keys = [
        "object-a.txt",
        "nested/object-b.txt",
        "cas-object.txt",
        "missing.txt",
    ];
    for key in keys {
        let _ = store.delete(key);
    }

    let mut metadata = BTreeMap::new();
    metadata.insert("Content-Type".to_string(), "text/plain".to_string());
    metadata.insert("x-amz-meta-wanix-live".to_string(), "true".to_string());
    store.put(
        "object-a.txt",
        Object::new(metadata.clone(), b"alpha".to_vec()),
    )?;
    store.put(
        "nested/object-b.txt",
        Object::new(metadata.clone(), b"beta".to_vec()),
    )?;

    let object = store
        .get("object-a.txt")?
        .expect("live object created by test exists");
    assert_eq!(object.body, b"alpha");
    assert!(
        header(&object.metadata, "x-amz-meta-wanix-live").is_some()
            || header(
                &object.metadata,
                "x-amz-meta-wanix-live".trim_start_matches("x-amz-meta-")
            )
            .is_some(),
        "live object metadata did not include wanix marker: {:?}",
        object.metadata
    );

    let listed = store.list_prefix("")?;
    let listed_keys: Vec<_> = listed.iter().map(|(key, _)| key.as_str()).collect();
    assert!(listed_keys.contains(&"/object-a.txt"));
    assert!(listed_keys.contains(&"/nested/object-b.txt"));

    assert!(store.compare_and_swap(
        "cas-object.txt",
        None,
        Some(Object::new(metadata.clone(), b"cas-1".to_vec())),
    )?);
    assert!(!store.compare_and_swap(
        "cas-object.txt",
        None,
        Some(Object::new(metadata.clone(), b"cas-2".to_vec())),
    )?);
    let expected = store
        .get("cas-object.txt")?
        .expect("CAS object created by live test exists");
    assert!(store.compare_and_swap(
        "cas-object.txt",
        Some(&expected),
        Some(Object::new(metadata, b"cas-2".to_vec())),
    )?);

    if live_enabled("WANIX_LIVE_S3_AUTH_FAILURE") {
        let bad_signer = AwsSigV4Signer::new("WANIX_BAD_ACCESS_KEY", "WANIX_BAD_SECRET", "auto");
        let bad_store = S3ObjectStore::with_signer(
            opt_env("WANIX_S3_ENDPOINT").unwrap(),
            opt_env("WANIX_S3_BUCKET").unwrap(),
            Some(prefix.clone()),
            Arc::new(NativeHttpTransport::new()),
            Arc::new(bad_signer),
        );
        assert!(bad_store.get("object-a.txt").is_err());
    }

    for key in ["object-a.txt", "nested/object-b.txt", "cas-object.txt"] {
        store.delete(key)?;
        assert!(store.get(key)?.is_none());
    }
    Ok(())
}

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn default_live_prefix() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("wanix-live/{millis}")
}
