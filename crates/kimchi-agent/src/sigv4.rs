//! AWS Signature Version 4, for Amazon Bedrock with access keys.
//!
//! Follows "Create a signed AWS API request" in the IAM user guide
//! (docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html): a
//! canonical request, the string to sign over its SHA-256, and an HMAC-SHA256 chain from the
//! secret key through the date, region and service. Services other than S3 take each path
//! segment URI-encoded twice in the canonical request, which matters for Bedrock model ids
//! (`…-v1:0` is sent as `…-v1%3A0` and signed as `…-v1%253A0`).

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Access keys, with the session token of temporary credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// One request to sign. `path` is as sent (already percent-encoded once); `query` is the raw
/// query string without `?`; `headers` are the ones to sign besides `host` and `x-amz-date`.
pub struct Request<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
}

/// What to add to the request: every `(name, value)` header, `authorization` included.
pub fn sign(req: &Request, creds: &Credentials, region: &str, service: &str, now: DateTime<Utc>) -> Vec<(String, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let mut headers: Vec<(String, String)> = req.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    headers.push(("host".into(), req.host.to_string()));
    headers.push(("x-amz-date".into(), amz_date.clone()));
    if let Some(t) = &creds.session_token {
        headers.push(("x-amz-security-token".into(), t.clone()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", collapse(v))).collect();
    let signed: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    let signed = signed.join(";");
    let canonical = canonical_request(req.method, &canonical_path(req.path), &canonical_query(req.query), &canonical_headers, &signed, &hex(&Sha256::digest(req.body)));
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let key = signing_key(&creds.secret_access_key, &date, region, service);
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    let mut out: Vec<(String, String)> = headers.into_iter().filter(|(k, _)| k != "host").collect();
    out.push(("authorization".into(), format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", creds.access_key_id)));
    out
}

fn canonical_request(method: &str, path: &str, query: &str, headers: &str, signed: &str, payload: &str) -> String {
    format!("{method}\n{path}\n{query}\n{headers}\n{signed}\n{payload}")
}

/// Sequential spaces in a header value become one.
fn collapse(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Each segment of the path as sent, encoded again.
fn canonical_path(path: &str) -> String {
    if path.is_empty() {
        return "/".into();
    }
    path.split('/').map(|s| encode(s, true)).collect::<Vec<_>>().join("/")
}

/// Parameters sorted by name, each name and value encoded (decoded first, so a value sent
/// encoded isn't encoded twice).
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (encode(&decode(k), true), encode(&decode(v), true))
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// RFC 3986 encoding as SigV4 wants it: unreserved characters stay, the rest is `%XX` in
/// capitals. `slash`: `/` is encoded too.
pub fn encode(s: &str, slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if !slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac(&k, region.as_bytes());
    let k = hmac(&k, service.as_bytes());
    hmac(&k, b"aws4_request")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// The worked example of the IAM user guide ("Examples of signed Signature Version 4
    /// requests" / the SigV4 test suite's `get-vanilla-query` style ListUsers request).
    #[test]
    fn matches_the_aws_documentation_example() {
        let creds = Credentials { access_key_id: "AKIDEXAMPLE".into(), secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(), session_token: None };
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let req = Request {
            method: "GET",
            host: "iam.amazonaws.com",
            path: "/",
            query: "Action=ListUsers&Version=2010-05-08",
            headers: &[("Content-Type", "application/x-www-form-urlencoded; charset=utf-8")],
            body: b"",
        };
        let headers = sign(&req, &creds, "us-east-1", "iam", now);
        let auth = &headers.iter().find(|(k, _)| k == "authorization").unwrap().1;
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
        assert!(headers.iter().any(|(k, v)| k == "x-amz-date" && v == "20150830T123600Z"));
    }

    /// `get-vanilla` and `get-vanilla-with-session-token` of the SigV4 test suite AWS publishes
    /// (github.com/awslabs/aws-c-auth, tests/aws-signing-test-suite/v4).
    #[test]
    fn matches_the_aws_signing_test_suite() {
        let mut creds = Credentials { access_key_id: "AKIDEXAMPLE".into(), secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(), session_token: None };
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let req = Request { method: "GET", host: "example.amazonaws.com", path: "/", query: "", headers: &[], body: b"" };
        let sig = |h: Vec<(String, String)>| h.into_iter().find(|(k, _)| k == "authorization").unwrap().1.rsplit("Signature=").next().unwrap().to_string();
        assert_eq!(sig(sign(&req, &creds, "us-east-1", "service", now)), "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31");
        creds.session_token = Some("6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267".into());
        assert_eq!(sig(sign(&req, &creds, "us-east-1", "service", now)), "07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8");
    }

    #[test]
    fn bedrock_model_ids_are_encoded_twice_in_the_canonical_path() {
        assert_eq!(canonical_path("/model/us.anthropic.claude-v1%3A0/converse-stream"), "/model/us.anthropic.claude-v1%253A0/converse-stream");
        assert_eq!(encode("a b/c:d", false), "a%20b/c%3Ad");
        assert_eq!(canonical_query("b=2&a=x%20y"), "a=x%20y&b=2");
    }

    #[test]
    fn session_tokens_are_signed() {
        let creds = Credentials { access_key_id: "AK".into(), secret_access_key: "s".into(), session_token: Some("tok".into()) };
        let req = Request { method: "POST", host: "bedrock-runtime.us-east-1.amazonaws.com", path: "/model/x/converse", query: "", headers: &[], body: b"{}" };
        let headers = sign(&req, &creds, "us-east-1", "bedrock", Utc::now());
        let auth = &headers.iter().find(|(k, _)| k == "authorization").unwrap().1;
        assert!(auth.contains("SignedHeaders=host;x-amz-date;x-amz-security-token"), "{auth}");
        assert!(headers.iter().any(|(k, v)| k == "x-amz-security-token" && v == "tok"));
    }
}
