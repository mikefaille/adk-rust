        #[cfg(feature = "vertex")]
        let request = if let GeminiBackend::Vertex(vertex) = &self.backend {
            let cached_headers = vertex
                .credentials
                .headers(Default::default())
                .await
                .context(GoogleCloudCredentialsFetchSnafu)?;
            let auth_headers = match cached_headers {
                google_cloud_auth::credentials::CacheableResource::New { data, .. } => data,
                google_cloud_auth::credentials::CacheableResource::NotModified => {
                    return Err(Error::BadResponse {
                        code: 500,
                        description: Some("Credentials returned NotModified without prior fetch".to_string()),
                    });
                }
            };
            let mut req = request;
            for (name, value) in auth_headers.iter() {
                req = req.header(name, value);
            }
            req
        } else {
            request
        };
