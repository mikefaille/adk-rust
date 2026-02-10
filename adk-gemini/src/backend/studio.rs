use crate::{
    backend::{GeminiBackend, BackendStream},
    client::{
        BadResponseSnafu, BadPartSnafu, ConstructUrlSnafu, DecodeResponseSnafu, DeserializeSnafu,
        InvalidApiKeySnafu, PerformRequestSnafu, ServiceAccountJwtSnafu, PerformRequestNewSnafu,
    },
    client::{Error, Model},
    generation::model::{GenerateContentRequest, GenerationResponse},
    embedding::model::{EmbedContentRequest, ContentEmbeddingResponse, BatchEmbedContentsRequest, BatchContentEmbeddingResponse},
    batch::model::{BatchOperation, BatchGenerateContentRequest, ListBatchesResponse},
    cache::model::{CachedContent, CreateCachedContentRequest, ListCachedContentsResponse, CacheExpirationRequest},
    files::model::{File, ListFilesResponse},
};
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt, Future};
use eventsource_stream::Eventsource;
use jsonwebtoken::{EncodingKey, Header};
use reqwest::{
    Client, ClientBuilder, RequestBuilder, Response,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt};
use std::{
    sync::{Arc, LazyLock},
};
use tokio::sync::Mutex;
use url::Url;

static DEFAULT_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://generativelanguage.googleapis.com/v1beta/")
        .expect("unreachable error: failed to parse default base URL")
});

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    token_uri: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedToken {
    access_token: String,
    expires_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceAccountTokenSource {
    key: ServiceAccountKey,
    scopes: Vec<String>,
    cached: Arc<Mutex<Option<CachedToken>>>,
}

impl ServiceAccountTokenSource {
    pub(crate) fn new(key: ServiceAccountKey) -> Self {
        Self {
            key,
            scopes: vec!["https://www.googleapis.com/auth/cloud-platform".to_string()],
            cached: Arc::new(Mutex::new(None)),
        }
    }

    async fn access_token(&self, http_client: &Client) -> Result<String, Error> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        {
            let cache = self.cached.lock().await;
            if let Some(token) = cache.as_ref() {
                if token.expires_at.saturating_sub(60) > now {
                    return Ok(token.access_token.clone());
                }
            }
        }

        let jwt = self.build_jwt(now)?;
        let token: CachedToken = self.fetch_token(http_client, jwt).await?;

        let mut cache = self.cached.lock().await;
        *cache = Some(token.clone());
        Ok(token.access_token)
    }

    fn build_jwt(&self, now: i64) -> Result<String, Error> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            scope: &'a str,
            aud: &'a str,
            iat: i64,
            exp: i64,
        }

        let exp = now + 3600;
        let scope = self.scopes.join(" ");
        let claims = Claims {
            iss: &self.key.client_email,
            scope: &scope,
            aud: &self.key.token_uri,
            iat: now,
            exp,
        };
        let encoding_key =
            EncodingKey::from_rsa_pem(self.key.private_key.as_bytes()).context(ServiceAccountJwtSnafu)?;
        jsonwebtoken::encode(&Header::new(jsonwebtoken::Algorithm::RS256), &claims, &encoding_key)
            .context(ServiceAccountJwtSnafu)
    }

    async fn fetch_token(&self, http_client: &Client, jwt: String) -> Result<CachedToken, Error> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: i64,
        }

        let url = &self.key.token_uri;
        let response = http_client
            .post(url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| Error::ServiceAccountToken { source: e, url: url.clone() })?;

        let response: Response = check_response(response).await?;
        let token: TokenResponse =
            response.json().await.context(DecodeResponseSnafu)?;
        let expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + token.expires_in;
        Ok(CachedToken { access_token: token.access_token, expires_at })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AuthConfig {
    ApiKey(String),
    ServiceAccount(ServiceAccountTokenSource),
}

#[derive(Debug)]
pub(crate) struct RestClient {
    pub(crate) http_client: Client,
    pub(crate) base_url: Url,
    pub(crate) auth: AuthConfig,
}

#[derive(Debug)]
pub struct StudioBackend {
    client: RestClient,
    model: Model,
}

impl StudioBackend {
    pub fn new(api_key: String, base_url: Option<Url>, model: Model) -> Result<Self, Error> {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.clone());
        let auth = AuthConfig::ApiKey(api_key.clone());

        let headers = HeaderMap::from_iter([(
            HeaderName::from_static("x-goog-api-key"),
            HeaderValue::from_str(&api_key).context(InvalidApiKeySnafu)?,
        )]);

        let http_client = ClientBuilder::new()
            .default_headers(headers)
            .build()
            .context(PerformRequestNewSnafu)?;

        Ok(Self {
            client: RestClient { http_client, base_url, auth },
            model,
        })
    }

    fn build_url(&self, endpoint: &str) -> Result<Url, Error> {
        let suffix = format!("models/{}:{}", self.model.as_str().trim_start_matches("models/"), endpoint);
        self.client.base_url.join(&suffix).context(ConstructUrlSnafu { suffix })
    }

    fn build_url_global(&self, endpoint: &str) -> Result<Url, Error> {
        self.client.base_url.join(endpoint).context(ConstructUrlSnafu { suffix: endpoint.to_string() })
    }

    async fn perform_request<
        T,
        F: FnOnce(&Client) -> RequestBuilder,
        G: FnOnce(Response) -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    >(
        &self,
        builder: F,
        checker: G,
    ) -> Result<T, Error> {
        let mut request_builder = builder(&self.client.http_client);

        if let AuthConfig::ServiceAccount(source) = &self.client.auth {
             let token = source.access_token(&self.client.http_client).await?;
             request_builder = request_builder.bearer_auth(token);
        }

        let request = request_builder.build().context(PerformRequestNewSnafu)?;
        let url = request.url().clone();

        let response = self.client.http_client.execute(request)
            .await
            .context(PerformRequestSnafu { url })?;

        checker(response).await
    }

    async fn post_json<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        url: Url,
        json: &Req,
    ) -> Result<Res, Error> {
        let response: Response = self.perform_request(|c| c.post(url).json(json), async |r| check_response(r).await)
            .await?;
        response.json::<Res>()
            .await
            .context(DecodeResponseSnafu)
    }

    // Helper for PATCH (update)
    async fn patch_json<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        url: Url,
        json: &Req,
    ) -> Result<Res, Error> {
        let response: Response = self.perform_request(|c| c.patch(url).json(json), async |r| check_response(r).await)
            .await?;
        response.json::<Res>()
            .await
            .context(DecodeResponseSnafu)
    }
}

async fn check_response(response: Response) -> Result<Response, Error> {
    let status = response.status();
    if !status.is_success() {
        let description = response.text().await.ok();
        BadResponseSnafu { code: status.as_u16(), description }.fail()
    } else {
        Ok(response)
    }
}

#[async_trait]
impl GeminiBackend for StudioBackend {
    async fn generate_content(&self, req: GenerateContentRequest) -> Result<GenerationResponse, Error> {
        let url = self.build_url("generateContent")?;
        self.post_json(url, &req).await
    }

    async fn generate_content_stream(&self, req: GenerateContentRequest) -> Result<BackendStream<GenerationResponse>, Error> {
        let mut url = self.build_url("streamGenerateContent")?;
        url.query_pairs_mut().append_pair("alt", "sse");

        let stream = self
            .perform_request(|c| c.post(url).json(&req), async |r| Ok(r.bytes_stream()))
            .await?;

        let stream = stream
            .eventsource()
            .map(|event| event.context(BadPartSnafu))
            .map_ok(|event| {
                serde_json::from_str::<GenerationResponse>(&event.data).context(DeserializeSnafu)
            })
            .map(|r| r.flatten());

        Ok(Box::pin(stream))
    }

    async fn embed_content(&self, req: EmbedContentRequest) -> Result<ContentEmbeddingResponse, Error> {
        let url = self.build_url("embedContent")?;
        self.post_json(url, &req).await
    }

    async fn batch_embed_contents(&self, req: BatchEmbedContentsRequest) -> Result<BatchContentEmbeddingResponse, Error> {
         let url = self.build_url("batchEmbedContents")?;
         self.post_json(url, &req).await
    }

    // Batch Operations
    async fn create_batch(&self, req: BatchGenerateContentRequest) -> Result<BatchOperation, Error> {
        // models/{model}:batchGenerateContent is likely correct for "create batch prediction job" in Rest API
        // if using Gemini API.
        // Or it might be `v1beta/batchGenerateContent`?
        // Checking `client.rs` in memory, it used `build_url("batchGenerateContent")`.
        // `build_url` appends `models/{model}:...`
        // So `models/{model}:batchGenerateContent`.
        let url = self.build_url("batchGenerateContent")?;
        self.post_json(url, &req).await
    }

    // Cache Operations
    async fn create_cached_content(&self, req: CreateCachedContentRequest) -> Result<CachedContent, Error> {
        let url = self.build_url_global("cachedContents")?;
        self.post_json(url, &req).await
    }

    async fn get_cached_content(&self, name: &str) -> Result<CachedContent, Error> {
        // name is likely "cachedContents/..." or just ID.
        // Usually full resource name.
        // If it starts with "cachedContents", use it directly.
        let url = if name.starts_with("cachedContents") {
             self.build_url_global(name)?
        } else {
             self.build_url_global(&format!("cachedContents/{}", name))?
        };

        let response: Response = self.perform_request(|c| c.get(url), async |r| check_response(r).await)
            .await?;
        response.json::<CachedContent>()
            .await
            .context(DecodeResponseSnafu)
    }

    async fn list_cached_contents(&self, page_size: Option<i32>, page_token: Option<String>) -> Result<ListCachedContentsResponse, Error> {
        let mut url = self.build_url_global("cachedContents")?;
        if let Some(ps) = page_size {
            url.query_pairs_mut().append_pair("pageSize", &ps.to_string());
        }
        if let Some(pt) = &page_token {
            url.query_pairs_mut().append_pair("pageToken", pt);
        }

        let response: Response = self.perform_request(|c| c.get(url), async |r| check_response(r).await)
            .await?;
        response.json::<ListCachedContentsResponse>()
            .await
            .context(DecodeResponseSnafu)
    }

    async fn update_cached_content(&self, name: &str, req: CacheExpirationRequest) -> Result<CachedContent, Error> {
        let url = if name.starts_with("cachedContents") {
             self.build_url_global(name)?
        } else {
             self.build_url_global(&format!("cachedContents/{}", name))?
        };

        self.patch_json(url, &req).await
    }

    async fn delete_cached_content(&self, name: &str) -> Result<(), Error> {
        let url = if name.starts_with("cachedContents") {
             self.build_url_global(name)?
        } else {
             self.build_url_global(&format!("cachedContents/{}", name))?
        };

        self.perform_request(|c| c.delete(url), async |r| check_response(r).await)
            .await?;
        Ok(())
    }

    // File Operations
    async fn list_files(&self, page_size: Option<u32>, page_token: Option<String>) -> Result<ListFilesResponse, Error> {
        let mut url = self.build_url_global("files")?;
        if let Some(ps) = page_size {
            url.query_pairs_mut().append_pair("pageSize", &ps.to_string());
        }
        if let Some(pt) = &page_token {
            url.query_pairs_mut().append_pair("pageToken", pt);
        }

        let response: Response = self.perform_request(|c| c.get(url), async |r| check_response(r).await)
            .await?;
        response.json::<ListFilesResponse>()
            .await
            .context(DecodeResponseSnafu)
    }

    async fn get_file(&self, name: &str) -> Result<File, Error> {
        let url = self.build_url_global(name)?;

        let response: Response = self.perform_request(|c| c.get(url), async |r| check_response(r).await)
            .await?;
        response.json::<File>()
            .await
            .context(DecodeResponseSnafu)
    }

    async fn delete_file(&self, name: &str) -> Result<(), Error> {
        let url = self.build_url_global(name)?;
        self.perform_request(|c| c.delete(url), async |r| check_response(r).await)
            .await?;
        Ok(())
    }
}
