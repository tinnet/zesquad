//! Putting objects in an S3 bucket: path-style URLs presigned by rusty-s3 (Signature Version
//! 4), sent with reqwest. Cloudflare R2, Hetzner Object Storage and AWS all accept them.
//!
//! A presigned URL is a credential until it expires: anyone holding it can make that one
//! request. So they're signed just before use, live briefly, and never end up in errors or logs.

use std::time::Duration;

use anyhow::{Context as _, bail};
use reqwest::{Url, header};
use rusty_s3::{Credentials, S3Action as _, UrlStyle};

/// For one upload of a few megabytes.
const PUT_TIMEOUT: Duration = Duration::from_mins(2);
/// How long a signed URL is valid. It's used at once; this only has to outlast the upload
/// starting, plus some clock skew.
const SIGNED_FOR: Duration = Duration::from_mins(5);

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
    s3: rusty_s3::Bucket,
    credentials: Credentials,
    /// Put in front of every key: the URL's path after the bucket, ending in `/` unless empty.
    prefix: String,
}

impl Bucket {
    /// # Errors
    ///
    /// When the URL has no host or no bucket, or the HTTP client fails to build.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        if config.url.host_str().is_none() {
            bail!("the archive URL {} has no host", config.url);
        }
        let mut segments = config
            .url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty());
        let Some(name) = segments.next().map(str::to_owned) else {
            bail!("the archive URL {} names no bucket", config.url);
        };
        let prefix = segments.fold(String::new(), |prefix, segment| prefix + segment + "/");
        let mut endpoint = config.url;
        endpoint.set_path("/");
        endpoint.set_query(None);
        let s3 = rusty_s3::Bucket::new(endpoint, UrlStyle::Path, name, config.region)
            .context("the archive URL")?;
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("zesquad/", env!("CARGO_PKG_VERSION")))
                .build()?,
            s3,
            credentials: Credentials::new(config.access_key_id, config.secret_access_key),
            prefix,
        })
    }

    /// A URL to PUT `key` (segments separated by `/`) with, valid for [`SIGNED_FOR`].
    fn signed_put(&self, key: &str) -> Url {
        let object = format!("{}{key}", self.prefix);
        self.s3
            .put_object(Some(&self.credentials), &object)
            .sign(SIGNED_FOR)
    }

    /// Puts `body` at `key` (segments separated by `/`), replacing any object there.
    ///
    /// # Errors
    ///
    /// On network errors or any answer but 2xx.
    pub async fn put(&self, key: &str, content_type: &str, body: Vec<u8>) -> anyhow::Result<()> {
        let response = self
            .http
            .put(self.signed_put(key))
            .timeout(PUT_TIMEOUT)
            .header(header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            // reqwest's errors carry the URL, which here is a credential.
            .map_err(reqwest::Error::without_url)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(url: &str) -> anyhow::Result<Bucket> {
        Bucket::new(Config {
            url: Url::parse(url).unwrap(),
            region: "auto".into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        })
    }

    fn query(url: &Url, name: &str) -> String {
        url.query_pairs()
            .find(|(n, _)| n == name)
            .map_or_else(String::new, |(_, v)| v.into_owned())
    }

    /// The signature itself is rusty-s3's (tested against AWS's examples there); here, that
    /// the bucket, prefix and key end up where they belong, and the secret doesn't.
    #[test]
    fn presigns_path_style_under_the_prefix() {
        for config in [
            "https://acct.r2.cloudflarestorage.com/archive/zesquad",
            "https://acct.r2.cloudflarestorage.com/archive/zesquad/",
        ] {
            let url = bucket(config)
                .unwrap()
                .signed_put("eu/1080/2026-11-20/x-items.json.zst");
            assert_eq!(url.host_str(), Some("acct.r2.cloudflarestorage.com"));
            assert_eq!(
                url.path(),
                "/archive/zesquad/eu/1080/2026-11-20/x-items.json.zst"
            );
            assert_eq!(query(&url, "X-Amz-Algorithm"), "AWS4-HMAC-SHA256");
            let credential = query(&url, "X-Amz-Credential");
            assert!(
                credential.starts_with("AKIAIOSFODNN7EXAMPLE/"),
                "{credential}"
            );
            assert!(
                credential.ends_with("/auto/s3/aws4_request"),
                "{credential}"
            );
            assert_eq!(query(&url, "X-Amz-Expires"), "300");
            assert_eq!(query(&url, "X-Amz-Signature").len(), 64);
            assert!(!url.as_str().contains("EXAMPLEKEY"));
        }
    }

    #[test]
    fn a_bucket_alone_has_no_prefix() {
        let url = bucket("https://s3.example/archive")
            .unwrap()
            .signed_put("a/b");
        assert_eq!(url.path(), "/archive/a/b");
    }

    #[test]
    fn needs_a_bucket() {
        assert!(bucket("https://s3.example/").is_err());
    }
}
