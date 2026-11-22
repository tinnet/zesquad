//! Just enough of S3 to put an object: path-style URLs and Signature Version 4, which
//! Cloudflare R2, Hetzner Object Storage and AWS all accept. A full SDK would be most of the
//! binary for one PUT.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, bail};
use hmac::{Hmac, KeyInit as _, Mac as _};
use reqwest::{Url, header};
use sha2::{Digest as _, Sha256};
use zesquad_core::archive::Utc;

use crate::auth::hex;

/// For one upload of a few megabytes.
const PUT_TIMEOUT: Duration = Duration::from_mins(2);

/// A bucket and how to sign in to it.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path-style: the endpoint, the bucket, and optionally a prefix for every key, e.g.
    /// `https://<account>.r2.cloudflarestorage.com/<bucket>`.
    pub url: Url,
    /// The signing region: `auto` for R2, the location (e.g. `fsn1`) for Hetzner.
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

pub struct Bucket {
    http: reqwest::Client,
    config: Config,
}

impl Bucket {
    /// # Errors
    ///
    /// When the URL has no host or the HTTP client fails to build.
    pub fn new(mut config: Config) -> anyhow::Result<Self> {
        if config.url.host_str().is_none() {
            bail!("the archive URL {} has no host", config.url);
        }
        // Keys are appended as path segments.
        if let Ok(mut segments) = config.url.path_segments_mut() {
            segments.pop_if_empty();
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("zesquad/", env!("CARGO_PKG_VERSION")))
                .build()?,
            config,
        })
    }

    /// Puts `body` at `key` (segments separated by `/`), replacing any object there.
    ///
    /// # Errors
    ///
    /// On network errors or any answer but 2xx.
    pub async fn put(&self, key: &str, content_type: &str, body: Vec<u8>) -> anyhow::Result<()> {
        let mut url = self.config.url.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("the archive URL can't have a path"))?
            .extend(key.split('/'));
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            (None, _) => bail!("the archive URL has no host"),
        };
        let unix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();
        let now = Utc::from_unix(i64::try_from(unix)?);
        let payload = hex(&Sha256::digest(&body));
        let amz_date = now.to_string();
        let authorization = authorization(
            &self.config,
            &now,
            "PUT",
            url.path(),
            &[
                ("host", &host),
                ("x-amz-content-sha256", &payload),
                ("x-amz-date", &amz_date),
            ],
            &payload,
        );
        let response = self
            .http
            .put(url)
            .timeout(PUT_TIMEOUT)
            .header(header::AUTHORIZATION, authorization)
            .header("x-amz-content-sha256", payload)
            .header("x-amz-date", amz_date)
            .header(header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            .with_context(|| format!("uploading {key}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!(
                "the archive answered {status} for {key}: {}",
                text.chars().take(300).collect::<String>()
            );
        }
        Ok(())
    }
}

/// The `Authorization` header for a request without a query string. `headers` are the signed
/// ones: lower-case names, sorted, and all sent as given.
fn authorization(
    config: &Config,
    now: &Utc,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
) -> String {
    let date = now.compact_date();
    let scope = format!("{date}/{}/s3/aws4_request", config.region);
    let signed: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
    let signed = signed.join(";");
    let canonical_headers = headers.iter().fold(String::new(), |mut s, (name, value)| {
        let _ = writeln!(s, "{name}:{}", value.trim());
        s
    });
    let canonical = format!("{method}\n{path}\n\n{canonical_headers}\n{signed}\n{payload_hash}");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{now}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let key = [date.as_str(), &config.region, "s3", "aws4_request"]
        .iter()
        .fold(
            format!("AWS4{}", config.secret_access_key).into_bytes(),
            |key, part| hmac(&key, part.as_bytes()),
        );
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={}",
        config.access_key_id,
        hex(&hmac(&key, to_sign.as_bytes()))
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The "GET Object" example in AWS's Signature Version 4 docs for S3 ("Examples:
    /// Signature Calculations in AWS Signature Version 4", header-based authentication).
    #[test]
    fn signs_like_the_aws_example() {
        let config = Config {
            url: Url::parse("https://examplebucket.s3.amazonaws.com").unwrap(),
            region: "us-east-1".into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        };
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let now = Utc::from_unix(1_369_353_600); // 2013-05-24T00:00:00Z
        assert_eq!(now.to_string(), "20130524T000000Z");
        assert_eq!(
            authorization(
                &config,
                &now,
                "GET",
                "/test.txt",
                &[
                    ("host", "examplebucket.s3.amazonaws.com"),
                    ("range", "bytes=0-9"),
                    ("x-amz-content-sha256", empty),
                    ("x-amz-date", "20130524T000000Z"),
                ],
                empty,
            ),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }
}
