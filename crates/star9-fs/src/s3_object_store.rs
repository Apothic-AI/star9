use std::collections::BTreeMap;
use std::sync::Arc;

use http::Method;

use crate::{Error, HttpRequest, HttpResponse, HttpTransport, Object, ObjectStore, Result};

pub trait HttpRequestSigner: Send + Sync {
    fn sign(&self, request: &mut HttpRequest) -> Result<()>;
}

impl<F> HttpRequestSigner for F
where
    F: Fn(&mut HttpRequest) -> Result<()> + Send + Sync,
{
    fn sign(&self, request: &mut HttpRequest) -> Result<()> {
        (self)(request)
    }
}

#[derive(Clone)]
pub struct S3ObjectStore {
    inner: Arc<S3ObjectStoreInner>,
}

struct S3ObjectStoreInner {
    base_url: String,
    bucket: String,
    prefix: Option<String>,
    transport: Arc<dyn HttpTransport>,
    signer: Arc<dyn HttpRequestSigner>,
}

#[derive(Default)]
struct NoopSigner;

impl HttpRequestSigner for NoopSigner {
    fn sign(&self, _request: &mut HttpRequest) -> Result<()> {
        Ok(())
    }
}

impl S3ObjectStore {
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        prefix: Option<String>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self::with_signer(base_url, bucket, prefix, transport, Arc::new(NoopSigner))
    }

    pub fn with_signer(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        prefix: Option<String>,
        transport: Arc<dyn HttpTransport>,
        signer: Arc<dyn HttpRequestSigner>,
    ) -> Self {
        Self {
            inner: Arc::new(S3ObjectStoreInner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                bucket: bucket.into().trim_matches('/').to_string(),
                prefix: normalize_prefix(prefix),
                transport,
                signer,
            }),
        }
    }

    fn bucket_url(&self) -> String {
        join_url_path(&self.inner.base_url, &percent_encode(&self.inner.bucket))
    }

    fn object_key(&self, key: &str) -> String {
        match self.inner.prefix.as_deref() {
            Some(prefix) if key.is_empty() || key == "/" => prefix.to_string(),
            Some(prefix) if key.starts_with('/') => format!("{prefix}{key}"),
            Some(prefix) => format!("{prefix}/{key}"),
            None if key.is_empty() => "/".to_string(),
            None => key.to_string(),
        }
    }

    fn external_key(&self, object_key: &str) -> Option<String> {
        let prefix = self.inner.prefix.as_deref()?;
        if object_key == prefix {
            return Some("/".to_string());
        }
        let suffix = object_key.strip_prefix(prefix)?;
        if suffix.is_empty() {
            Some("/".to_string())
        } else if suffix.starts_with('/') {
            Some(suffix.to_string())
        } else {
            Some(format!("/{suffix}"))
        }
    }

    fn request(
        &self,
        method: Method,
        url: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse> {
        let mut request = HttpRequest {
            method,
            url,
            headers,
            body,
        };
        self.inner.signer.sign(&mut request)?;
        self.inner.transport.request(request)
    }

    fn request_object(
        &self,
        method: Method,
        key: &str,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse> {
        let object_key = self.object_key(key);
        let url = join_url_path(&self.bucket_url(), &percent_encode(&object_key));
        self.request(method, url, headers, body)
    }

    fn request_effective_object(
        &self,
        method: Method,
        object_key: &str,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse> {
        let url = join_url_path(&self.bucket_url(), &percent_encode(object_key));
        self.request(method, url, headers, body)
    }

    fn request_list(&self, prefix: &str) -> Result<HttpResponse> {
        let object_prefix = self.object_key(prefix);
        let url = format!(
            "{}?list-type=2&prefix={}",
            self.bucket_url(),
            percent_encode(&object_prefix)
        );
        self.request(Method::GET, url, BTreeMap::new(), Vec::new())
    }

    fn unexpected_status(&self, operation: &str, target: &str, status: u16) -> Error {
        Error::Message(format!(
            "s3 object store {operation} {target} returned HTTP {status}"
        ))
    }
}

impl ObjectStore for S3ObjectStore {
    fn get(&self, key: &str) -> Result<Option<Object>> {
        let response = self.request_object(Method::GET, key, BTreeMap::new(), Vec::new())?;
        match response.status {
            200..=299 => Ok(Some(Object::new(response.headers, response.body))),
            404 => Ok(None),
            status => Err(self.unexpected_status("get", key, status)),
        }
    }

    fn put(&self, key: &str, object: Object) -> Result<()> {
        let response = self.request_object(
            Method::PUT,
            key,
            object.metadata.clone(),
            object.body.clone(),
        )?;
        match response.status {
            200..=299 => Ok(()),
            status => Err(self.unexpected_status("put", key, status)),
        }
    }

    fn compare_and_swap(
        &self,
        key: &str,
        expected: Option<&Object>,
        new: Option<Object>,
    ) -> Result<bool> {
        let mut headers = new
            .as_ref()
            .map(|object| object.metadata.clone())
            .unwrap_or_default();

        match expected {
            Some(object) => {
                let Some(etag) = header_value(&object.metadata, "etag") else {
                    return Ok(false);
                };
                headers.insert("If-Match".to_string(), etag.to_string());
            }
            None => {
                headers.insert("If-None-Match".to_string(), "*".to_string());
            }
        }

        let (method, body) = match new {
            Some(object) => (Method::PUT, object.body),
            None => (Method::DELETE, Vec::new()),
        };

        let response = self.request_object(method, key, headers, body)?;
        match response.status {
            200..=299 => Ok(true),
            404 | 409 | 412 => Ok(false),
            status => Err(self.unexpected_status("compare-and-swap", key, status)),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        let response = self.request_object(Method::DELETE, key, BTreeMap::new(), Vec::new())?;
        match response.status {
            200..=299 | 404 => Ok(()),
            status => Err(self.unexpected_status("delete", key, status)),
        }
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<(String, Object)>> {
        let response = self.request_list(prefix)?;
        if !(200..=299).contains(&response.status) {
            return Err(self.unexpected_status("list-prefix", prefix, response.status));
        }

        let mut objects = Vec::new();
        for object_key in parse_list_objects_v2_keys(&response.body)? {
            let external_key = self
                .external_key(&object_key)
                .unwrap_or_else(|| object_key.clone());
            let response = self.request_effective_object(
                Method::GET,
                &object_key,
                BTreeMap::new(),
                Vec::new(),
            )?;
            match response.status {
                200..=299 => {
                    objects.push((external_key, Object::new(response.headers, response.body)));
                }
                status => {
                    return Err(self.unexpected_status("list-prefix-fetch", &external_key, status));
                }
            }
        }
        Ok(objects)
    }
}

fn normalize_prefix(prefix: Option<String>) -> Option<String> {
    let prefix = prefix?;
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("/{trimmed}"))
    }
}

fn join_url_path(base: &str, segment: &str) -> String {
    if base.is_empty() {
        return segment.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        segment.trim_start_matches('/')
    )
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
        ) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn parse_list_objects_v2_keys(body: &[u8]) -> Result<Vec<String>> {
    let xml = String::from_utf8_lossy(body);
    let mut keys = Vec::new();
    let mut remaining = xml.as_ref();

    while let Some(start) = remaining.find("<Contents") {
        let contents = &remaining[start..];
        let Some((block, rest)) = extract_tag_body(contents, "Contents") else {
            return Err(Error::Message(
                "s3 object store returned malformed ListObjectsV2 XML".to_string(),
            ));
        };
        let Some((key, _)) = extract_tag_body(block, "Key") else {
            return Err(Error::Message(
                "s3 object store returned ListObjectsV2 XML without a Contents Key".to_string(),
            ));
        };
        keys.push(xml_unescape(key)?);
        remaining = rest;
    }

    Ok(keys)
}

fn extract_tag_body<'a>(xml: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after_open = &xml[start + open.len()..];
    let open_end = after_open.find('>')?;
    let content_start = start + open.len() + open_end + 1;
    let close = format!("</{tag}>");
    let content_end = content_start + xml[content_start..].find(&close)?;
    let content = &xml[content_start..content_end];
    let rest = &xml[content_end + close.len()..];
    Some((content, rest))
}

fn xml_unescape(value: &str) -> Result<String> {
    let mut unescaped = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            unescaped.push(ch);
            continue;
        }

        let mut entity = String::new();
        for next in chars.by_ref() {
            if next == ';' {
                break;
            }
            entity.push(next);
        }

        match entity.as_str() {
            "amp" => unescaped.push('&'),
            "lt" => unescaped.push('<'),
            "gt" => unescaped.push('>'),
            "apos" => unescaped.push('\''),
            "quot" => unescaped.push('"'),
            _ => {
                return Err(Error::Message(format!(
                    "s3 object store returned unsupported XML entity &{entity};"
                )));
            }
        }
    }
    Ok(unescaped)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{HttpRequestSigner, S3ObjectStore};
    use crate::{Error, HttpRequest, HttpResponse, HttpTransport, Object, ObjectStore, Result};

    #[derive(Default)]
    struct RecordingTransport {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl RecordingTransport {
        fn push(&self, response: HttpResponse) {
            self.responses.lock().unwrap().push_back(response);
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HttpTransport for RecordingTransport {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| Error::Message("missing test response".to_string()))
        }
    }

    #[test]
    fn object_store_get_found_preserves_headers_and_body() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(
            HttpResponse::new(200)
                .with_header("ETag", "\"abc\"")
                .with_header("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT")
                .with_body(b"hello".to_vec()),
        );
        let store = S3ObjectStore::new(
            "https://objects.example/api/",
            "bucket",
            None,
            transport.clone(),
        );

        let object = store.get("/hello.txt").unwrap().unwrap();

        assert_eq!(
            object.metadata.get("ETag").map(String::as_str),
            Some("\"abc\"")
        );
        assert_eq!(
            object.metadata.get("Last-Modified").map(String::as_str),
            Some("Wed, 21 Oct 2015 07:28:00 GMT")
        );
        assert_eq!(object.body, b"hello");

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, http::Method::GET);
        assert_eq!(
            requests[0].url,
            "https://objects.example/api/bucket/%2Fhello.txt"
        );
    }

    #[test]
    fn object_store_get_not_found_returns_none() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(404));
        let store = S3ObjectStore::new("https://objects.example", "bucket", None, transport);

        assert_eq!(store.get("/missing").unwrap(), None);
    }

    #[test]
    fn object_store_put_preserves_metadata_and_body() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(200));
        let store =
            S3ObjectStore::new("https://objects.example", "bucket", None, transport.clone());
        let object = Object::new(
            BTreeMap::from([
                (
                    "Content-Type".to_string(),
                    "application/octet-stream".to_string(),
                ),
                ("X-Object-Meta".to_string(), "1".to_string()),
            ]),
            b"payload".to_vec(),
        );

        store.put("/data.bin", object).unwrap();

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, http::Method::PUT);
        assert_eq!(
            requests[0].url,
            "https://objects.example/bucket/%2Fdata.bin"
        );
        assert_eq!(
            requests[0].headers,
            BTreeMap::from([
                (
                    "Content-Type".to_string(),
                    "application/octet-stream".to_string()
                ),
                ("X-Object-Meta".to_string(), "1".to_string()),
            ])
        );
        assert_eq!(requests[0].body, b"payload");
    }

    #[test]
    fn object_store_delete_404_is_ok() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(404));
        let store = S3ObjectStore::new("https://objects.example", "bucket", None, transport);

        store.delete("/missing").unwrap();
    }

    #[test]
    fn object_store_list_prefix_parses_xml_and_fetches_each_object() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(
            HttpResponse::new(200).with_body(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Contents><Key>/tenant/root/docs/a.txt</Key></Contents>
  <Contents><Key>/tenant/root/docs/b&amp;c.txt</Key></Contents>
</ListBucketResult>"#
                    .to_vec(),
            ),
        );
        transport.push(
            HttpResponse::new(200)
                .with_header("ETag", "\"a\"")
                .with_body(b"a".to_vec()),
        );
        transport.push(
            HttpResponse::new(200)
                .with_header("ETag", "\"b\"")
                .with_body(b"b".to_vec()),
        );
        let store = S3ObjectStore::new(
            "https://objects.example",
            "bucket",
            Some("tenant/root".to_string()),
            transport.clone(),
        );

        let objects = store.list_prefix("/docs").unwrap();

        assert_eq!(
            objects,
            vec![
                (
                    "/docs/a.txt".to_string(),
                    Object::new(
                        BTreeMap::from([("ETag".to_string(), "\"a\"".to_string())]),
                        b"a".to_vec(),
                    ),
                ),
                (
                    "/docs/b&c.txt".to_string(),
                    Object::new(
                        BTreeMap::from([("ETag".to_string(), "\"b\"".to_string())]),
                        b"b".to_vec(),
                    ),
                ),
            ]
        );

        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0].url,
            "https://objects.example/bucket?list-type=2&prefix=%2Ftenant%2Froot%2Fdocs"
        );
        assert_eq!(
            requests[1].url,
            "https://objects.example/bucket/%2Ftenant%2Froot%2Fdocs%2Fa.txt"
        );
        assert_eq!(
            requests[2].url,
            "https://objects.example/bucket/%2Ftenant%2Froot%2Fdocs%2Fb%26c.txt"
        );
    }

    #[test]
    fn object_store_compare_and_swap_uses_if_match_etag() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(200));
        let store =
            S3ObjectStore::new("https://objects.example", "bucket", None, transport.clone());
        let expected = Object::new(
            BTreeMap::from([("ETag".to_string(), "\"etag-1\"".to_string())]),
            Vec::new(),
        );
        let new_object = Object::new(
            BTreeMap::from([("Content-Type".to_string(), "text/plain".to_string())]),
            b"next".to_vec(),
        );

        assert!(store
            .compare_and_swap("/doc.txt", Some(&expected), Some(new_object))
            .unwrap());

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, http::Method::PUT);
        assert_eq!(
            requests[0].headers.get("If-Match").map(String::as_str),
            Some("\"etag-1\"")
        );
        assert_eq!(
            requests[0].headers.get("Content-Type").map(String::as_str),
            Some("text/plain")
        );
        assert_eq!(requests[0].body, b"next");
    }

    #[test]
    fn object_store_compare_and_swap_missing_etag_returns_false_without_request() {
        let transport = Arc::new(RecordingTransport::default());
        let store =
            S3ObjectStore::new("https://objects.example", "bucket", None, transport.clone());
        let expected = Object::new(BTreeMap::new(), Vec::new());
        let new_object = Object::new(BTreeMap::new(), b"next".to_vec());

        assert!(!store
            .compare_and_swap("/doc.txt", Some(&expected), Some(new_object))
            .unwrap());
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn object_store_compare_and_swap_expected_none_uses_if_none_match_star() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(200));
        let store =
            S3ObjectStore::new("https://objects.example", "bucket", None, transport.clone());
        let new_object = Object::new(
            BTreeMap::from([("Content-Type".to_string(), "text/plain".to_string())]),
            b"created".to_vec(),
        );

        assert!(store
            .compare_and_swap("/new.txt", None, Some(new_object))
            .unwrap());

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers.get("If-None-Match").map(String::as_str),
            Some("*")
        );
    }

    #[test]
    fn object_store_signer_hook_is_invoked_before_dispatch() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(200));
        let invoked = Arc::new(AtomicUsize::new(0));
        let invoked_for_signer = invoked.clone();
        let signer: Arc<dyn HttpRequestSigner> = Arc::new(move |request: &mut HttpRequest| {
            invoked_for_signer.fetch_add(1, Ordering::SeqCst);
            request
                .headers
                .insert("Authorization".to_string(), "Signed test".to_string());
            Ok(())
        });
        let store = S3ObjectStore::with_signer(
            "https://objects.example",
            "bucket",
            None,
            transport.clone(),
            signer,
        );

        store.get("/signed.txt").unwrap();

        assert_eq!(invoked.load(Ordering::SeqCst), 1);
        let requests = transport.requests();
        assert_eq!(
            requests[0].headers.get("Authorization").map(String::as_str),
            Some("Signed test")
        );
    }
}
