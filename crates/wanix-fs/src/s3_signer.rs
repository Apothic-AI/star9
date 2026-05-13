use std::collections::BTreeMap;
use std::time::SystemTime;

use hmac::{Hmac, Mac};
use http::Uri;
use sha2::{Digest, Sha256};

use crate::{Error, HttpRequest, HttpRequestSigner, Result};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct AwsSigV4Signer {
    access_key_id: String,
    secret_access_key: String,
    region: String,
    service: String,
    signing_time: Option<SystemTime>,
}

impl AwsSigV4Signer {
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self::with_service(access_key_id, secret_access_key, region, "s3")
    }

    pub fn with_service(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        region: impl Into<String>,
        service: impl Into<String>,
    ) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            region: region.into(),
            service: service.into(),
            signing_time: None,
        }
    }

    pub fn with_signing_time(mut self, signing_time: SystemTime) -> Self {
        self.signing_time = Some(signing_time);
        self
    }

    fn signing_time(&self) -> SystemTime {
        self.signing_time.unwrap_or_else(SystemTime::now)
    }
}

impl HttpRequestSigner for AwsSigV4Signer {
    fn sign(&self, request: &mut HttpRequest) -> Result<()> {
        let uri: Uri = request
            .url
            .parse()
            .map_err(|err| Error::Message(format!("invalid S3 signing URL: {err}")))?;
        let authority = uri
            .authority()
            .ok_or_else(|| Error::Message("S3 signing URL missing authority".to_string()))?;
        let host = authority.as_str().to_string();
        let payload_hash = hex_sha256(&request.body);
        let (amz_date, short_date) = format_amz_date(self.signing_time())?;

        set_header_case_insensitive(&mut request.headers, "host", host.clone());
        set_header_case_insensitive(&mut request.headers, "x-amz-date", amz_date.clone());
        set_header_case_insensitive(
            &mut request.headers,
            "x-amz-content-sha256",
            payload_hash.clone(),
        );
        remove_header_case_insensitive(&mut request.headers, "authorization");

        let canonical_uri = canonical_uri(uri.path());
        let canonical_query = canonical_query(uri.query().unwrap_or_default());
        let (canonical_headers, signed_headers) = canonical_headers(&request.headers);
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            request.method.as_str(),
            canonical_uri,
            canonical_query,
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let credential_scope = format!(
            "{}/{}/{}/aws4_request",
            short_date, self.region, self.service
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            hex_sha256(canonical_request.as_bytes())
        );
        let signature = hex_encode(&hmac_sign(
            &signing_key(
                self.secret_access_key.as_bytes(),
                short_date.as_bytes(),
                self.region.as_bytes(),
                self.service.as_bytes(),
            )?,
            string_to_sign.as_bytes(),
        )?);
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{},SignedHeaders={},Signature={}",
            self.access_key_id, credential_scope, signed_headers, signature
        );
        request
            .headers
            .insert("Authorization".to_string(), authorization);
        Ok(())
    }
}

fn canonical_uri(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    aws_encode(path.as_bytes(), true)
}

fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }

    let mut params = Vec::new();
    for part in query.split('&') {
        let (name, value) = match part.split_once('=') {
            Some((name, value)) => (name, value),
            None => (part, ""),
        };
        params.push((
            aws_encode(name.as_bytes(), false),
            aws_encode(value.as_bytes(), false),
        ));
    }
    params.sort();
    params
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn canonical_headers(headers: &BTreeMap<String, String>) -> (String, String) {
    let mut canonical = BTreeMap::<String, Vec<String>>::new();
    for (name, value) in headers {
        canonical
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(normalize_header_value(value));
    }

    let mut header_lines = Vec::new();
    let mut signed_names = Vec::new();
    for (name, values) in canonical {
        header_lines.push(format!("{name}:{}", values.join(",")));
        signed_names.push(name);
    }
    let canonical_headers = format!("{}\n", header_lines.join("\n"));
    let signed_headers = signed_names.join(";");
    (canonical_headers, signed_headers)
}

fn normalize_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn set_header_case_insensitive(headers: &mut BTreeMap<String, String>, name: &str, value: String) {
    remove_header_case_insensitive(headers, name);
    headers.insert(name.to_string(), value);
}

fn remove_header_case_insensitive(headers: &mut BTreeMap<String, String>, name: &str) {
    let matches = headers
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(name))
        .cloned()
        .collect::<Vec<_>>();
    for key in matches {
        headers.remove(&key);
    }
}

fn aws_encode(bytes: &[u8], keep_slash: bool) -> String {
    let mut encoded = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && index + 2 < bytes.len()
            && is_hex_digit(bytes[index + 1])
            && is_hex_digit(bytes[index + 2])
        {
            encoded.push('%');
            encoded.push((bytes[index + 1] as char).to_ascii_uppercase());
            encoded.push((bytes[index + 2] as char).to_ascii_uppercase());
            index += 3;
            continue;
        }
        if is_unreserved(byte) || (keep_slash && byte == b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
        index += 1;
    }
    encoded
}

fn is_unreserved(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~')
}

fn is_hex_digit(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

fn hex_sha256(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn signing_key(
    secret_access_key: &[u8],
    short_date: &[u8],
    region: &[u8],
    service: &[u8],
) -> Result<Vec<u8>> {
    let mut secret = b"AWS4".to_vec();
    secret.extend_from_slice(secret_access_key);
    let date_key = hmac_sign(&secret, short_date)?;
    let region_key = hmac_sign(&date_key, region)?;
    let service_key = hmac_sign(&region_key, service)?;
    hmac_sign(&service_key, b"aws4_request")
}

fn hmac_sign(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|err| Error::Message(format!("invalid S3 signing key: {err}")))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn format_amz_date(time: SystemTime) -> Result<(String, String)> {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| Error::Message("S3 signing time predates Unix epoch".to_string()))?;
    let secs = duration.as_secs();
    let days = (secs / 86_400) as i64;
    let seconds_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let short_date = format!("{year:04}{month:02}{day:02}");
    let amz_date = format!("{short_date}T{hour:02}{minute:02}{second:02}Z");
    Ok((amz_date, short_date))
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime};

    use http::Method;

    use super::AwsSigV4Signer;
    use crate::{HttpRequest, HttpRequestSigner};

    fn aws_example_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_369_353_600)
    }

    #[test]
    fn signer_matches_aws_get_object_reference_vector() {
        let signer = AwsSigV4Signer::new(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
        )
        .with_signing_time(aws_example_time());
        let mut request = HttpRequest {
            method: Method::GET,
            url: "https://examplebucket.s3.amazonaws.com/test.txt".to_string(),
            headers: BTreeMap::from([("Range".to_string(), "bytes=0-9".to_string())]),
            body: Vec::new(),
        };

        signer.sign(&mut request).unwrap();

        assert_eq!(
            request.headers.get("host").map(String::as_str),
            Some("examplebucket.s3.amazonaws.com")
        );
        assert_eq!(
            request.headers.get("x-amz-date").map(String::as_str),
            Some("20130524T000000Z")
        );
        assert_eq!(
            request
                .headers
                .get("x-amz-content-sha256")
                .map(String::as_str),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41")
        );
    }

    #[test]
    fn signer_matches_aws_get_bucket_lifecycle_reference_vector() {
        let signer = AwsSigV4Signer::new(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
        )
        .with_signing_time(aws_example_time());
        let mut request = HttpRequest {
            method: Method::GET,
            url: "https://examplebucket.s3.amazonaws.com/?lifecycle".to_string(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        };

        signer.sign(&mut request).unwrap();

        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;x-amz-content-sha256;x-amz-date,Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543")
        );
    }

    #[test]
    fn signer_is_deterministic_and_includes_existing_headers_in_signature() {
        let signer = AwsSigV4Signer::new(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
        )
        .with_signing_time(aws_example_time());
        let headers = BTreeMap::from([
            (
                "Date".to_string(),
                "Fri, 24 May 2013 00:00:00 GMT".to_string(),
            ),
            (
                "x-amz-storage-class".to_string(),
                "REDUCED_REDUNDANCY".to_string(),
            ),
        ]);
        let mut request_a = HttpRequest {
            method: Method::PUT,
            url: "https://examplebucket.s3.amazonaws.com/test$file.text".to_string(),
            headers: headers.clone(),
            body: b"Welcome to Amazon S3.".to_vec(),
        };
        let mut request_b = request_a.clone();

        signer.sign(&mut request_a).unwrap();
        signer.sign(&mut request_b).unwrap();

        assert_eq!(
            request_a.headers.get("Authorization"),
            request_b.headers.get("Authorization")
        );
        assert_eq!(
            request_a.headers.get("Authorization").map(String::as_str),
            Some("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class,Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd")
        );
    }
}
